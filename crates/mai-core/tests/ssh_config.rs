use std::path::{Path, PathBuf};

use mai_core::ssh::config::{config_warnings, parse_config, resolve};

const CONFIG: &str = "\
Host mac
  HostName 100.96.237.7
  User cyt

Host box
  HostName box.internal
  Port 2222
  IdentityFile ~/.ssh/box_key
  ProxyJump mac
  ForwardX11Trusted yes

Host loop-a
  ProxyJump loop-b

Host loop-b
  ProxyJump loop-a

Host *
  User fallback
";

fn home() -> PathBuf {
    PathBuf::from("/home/me")
}

fn default_keys(home: &Path) -> Vec<PathBuf> {
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .iter()
        .map(|k| home.join(".ssh").join(k))
        .collect()
}

#[test]
fn alias_uses_config_values_and_default_keys() {
    let cfg = parse_config(CONFIG).unwrap();
    let spec = resolve(&cfg, "mac", "local", &home()).unwrap();
    assert_eq!(spec.alias, "mac");
    assert_eq!(spec.host, "100.96.237.7");
    assert_eq!(spec.port, 22);
    assert_eq!(spec.user, "cyt");
    assert_eq!(spec.identity_files, default_keys(&home()));
    assert!(spec.jumps.is_empty());
}

#[test]
fn port_identity_jump_and_wildcard_user() {
    let cfg = parse_config(CONFIG).unwrap();
    let spec = resolve(&cfg, "box", "local", &home()).unwrap();
    assert_eq!((spec.host.as_str(), spec.port), ("box.internal", 2222));
    assert_eq!(spec.user, "fallback");
    assert_eq!(spec.identity_files.len(), 1);
    assert!(
        spec.identity_files[0].is_absolute(),
        "{:?}",
        spec.identity_files
    );
    assert!(spec.identity_files[0].ends_with(Path::new(".ssh").join("box_key")));
    assert_eq!(spec.jumps.len(), 1);
    assert_eq!(spec.jumps[0].host, "100.96.237.7");
    assert_eq!(spec.jumps[0].user, "cyt");
}

#[test]
fn explicit_user_host_port() {
    let cfg = parse_config(CONFIG).unwrap();
    let spec = resolve(&cfg, "root@10.0.0.5:2200", "local", &home()).unwrap();
    assert_eq!(
        (spec.user.as_str(), spec.host.as_str(), spec.port),
        ("root", "10.0.0.5", 2200)
    );
    let spec = resolve(&cfg, "ops@mac", "local", &home()).unwrap();
    assert_eq!(
        (spec.user.as_str(), spec.host.as_str()),
        ("ops", "100.96.237.7")
    );
}

#[test]
fn ipv6_forms() {
    let cfg = parse_config("").unwrap();
    let spec = resolve(&cfg, "[fe80::1]:2022", "me", &home()).unwrap();
    assert_eq!((spec.host.as_str(), spec.port), ("fe80::1", 2022));
    let spec = resolve(&cfg, "fe80::1", "me", &home()).unwrap();
    assert_eq!((spec.host.as_str(), spec.port), ("fe80::1", 22));
}

#[test]
fn unknown_alias_falls_back_to_name_and_default_user() {
    let cfg = parse_config("").unwrap();
    let spec = resolve(&cfg, "plain.host", "me", &home()).unwrap();
    assert_eq!(
        (spec.host.as_str(), spec.user.as_str()),
        ("plain.host", "me")
    );
}

#[test]
fn jump_hosts_own_proxyjump_is_not_followed() {
    let cfg = parse_config(CONFIG).unwrap();
    let spec = resolve(&cfg, "loop-a", "me", &home()).unwrap();
    assert_eq!(spec.jumps.len(), 1);
    assert_eq!(spec.jumps[0].host, "loop-b");
    assert!(spec.jumps[0].jumps.is_empty());
}

const WITH_MATCH: &str = "\
Host a
  User alice
Match host a exec \"true\"
  User mallory
  Port 2222
Host b
  User bob
";

#[test]
fn match_blocks_are_ignored_not_misattributed() {
    let cfg = parse_config(WITH_MATCH).unwrap();
    let a = resolve(&cfg, "a", "me", &home()).unwrap();
    assert_eq!((a.user.as_str(), a.port), ("alice", 22));
    let b = resolve(&cfg, "b", "me", &home()).unwrap();
    assert_eq!(b.user, "bob");
}

#[test]
fn match_blocks_are_reported() {
    assert_eq!(
        config_warnings(WITH_MATCH),
        vec![
            "ssh config line 3: Match blocks are not supported; their settings are ignored"
                .to_owned()
        ]
    );
    assert!(config_warnings(CONFIG).is_empty());
}

#[test]
fn bad_port_is_an_error() {
    let cfg = parse_config("").unwrap();
    assert!(resolve(&cfg, "host:notaport", "me", &home()).is_err());
}
