use std::ffi::OsString;

use mai_probe::zellij::{
    find_zellij, parse_panes, parse_sessions, parse_version, sessions_from_output,
};
use mai_protocol::{PaneInfo, SessionInfo};

#[test]
fn sessions_parse_names_and_exited_flag() {
    let text = "work [Created 2months 6days ago] (current)\n\
                mai-spike-u5 [Created 14m 46s ago] (EXITED - attach to resurrect)\n\
                \n";
    assert_eq!(
        parse_sessions(text),
        vec![
            SessionInfo {
                name: "work".into(),
                exited: false
            },
            SessionInfo {
                name: "mai-spike-u5".into(),
                exited: true
            },
        ]
    );
}

#[test]
fn no_sessions_message_means_empty_list() {
    let msg = "No active zellij sessions found.\n";
    assert_eq!(sessions_from_output(false, msg, ""), Ok(vec![]));
    assert_eq!(sessions_from_output(true, msg, ""), Ok(vec![]));
    assert_eq!(sessions_from_output(false, "", msg), Ok(vec![]));
}

#[test]
fn sessions_output_success_parses_and_failure_is_error() {
    assert_eq!(
        sessions_from_output(true, "work [Created 1h ago] (current)\n", ""),
        Ok(vec![SessionInfo {
            name: "work".into(),
            exited: false
        }])
    );
    let err = sessions_from_output(false, "", "permission denied").unwrap_err();
    assert!(err.0.contains("permission denied"), "{err}");
}

#[test]
fn pane_with_only_required_fields_parses() {
    let panes = parse_panes(r#"[{"id": 2, "is_plugin": false}]"#).unwrap();
    assert_eq!(panes.len(), 1);
    assert_eq!(panes[0].id, 2);
    assert_eq!(panes[0].title, "");
    assert!(!panes[0].exited);
}

/// Shape of `zellij action list-panes -a -j` (zellij 0.44.3), trimmed to
/// one plugin, one idle shell and one pane running a command.
const PANES_JSON: &str = r#"[
  {"id": 0, "is_plugin": true, "is_focused": false, "is_fullscreen": false,
   "is_floating": false, "is_suppressed": false, "title": "zellij:tab-bar",
   "exited": false, "exit_status": null, "is_held": false, "pane_x": 0,
   "pane_content_x": 0, "pane_y": 0, "pane_content_y": 0, "pane_rows": 1,
   "pane_content_rows": 1, "pane_columns": 80, "pane_content_columns": 80,
   "cursor_coordinates_in_pane": null, "terminal_command": null,
   "plugin_url": "zellij:tab-bar", "is_selectable": false,
   "index_in_pane_group": {}, "default_fg": null, "default_bg": null,
   "tab_id": 0, "tab_position": 0, "tab_name": "main"},
  {"id": 3, "is_plugin": false, "is_focused": true, "is_fullscreen": false,
   "is_floating": false, "is_suppressed": false,
   "title": "C:\\WINDOWS\\system32\\cmd.exe", "exited": false,
   "exit_status": null, "is_held": false, "pane_x": 0, "pane_content_x": 1,
   "pane_y": 1, "pane_content_y": 2, "pane_rows": 20, "pane_content_rows": 18,
   "pane_columns": 80, "pane_content_columns": 78,
   "cursor_coordinates_in_pane": [0, 0], "terminal_command": null,
   "plugin_url": null, "is_selectable": true, "index_in_pane_group": {},
   "default_fg": null, "default_bg": null, "tab_id": 0, "tab_position": 0,
   "tab_name": "main", "pane_cwd": "C:\\work"},
  {"id": 14, "is_plugin": false, "is_focused": false, "is_fullscreen": false,
   "is_floating": false, "is_suppressed": false, "title": "Claude Code",
   "exited": false, "exit_status": null, "is_held": false, "pane_x": 0,
   "pane_content_x": 1, "pane_y": 21, "pane_content_y": 22, "pane_rows": 20,
   "pane_content_rows": 18, "pane_columns": 80, "pane_content_columns": 78,
   "cursor_coordinates_in_pane": null,
   "terminal_command": "C:\\WINDOWS\\system32\\cmd.exe",
   "plugin_url": null, "is_selectable": true, "index_in_pane_group": {},
   "default_fg": null, "default_bg": null, "tab_id": 2, "tab_position": 1,
   "tab_name": "agents", "pane_command": "claude.exe -c",
   "pane_cwd": "G:\\work"}
]"#;

#[test]
fn panes_skip_plugins_and_prefer_running_command() {
    assert_eq!(
        parse_panes(PANES_JSON).unwrap(),
        vec![
            PaneInfo {
                id: 3,
                tab_id: 0,
                tab_name: "main".into(),
                title: "C:\\WINDOWS\\system32\\cmd.exe".into(),
                command: None,
                exited: false,
            },
            PaneInfo {
                id: 14,
                tab_id: 2,
                tab_name: "agents".into(),
                title: "Claude Code".into(),
                command: Some("claude.exe -c".into()),
                exited: false,
            },
        ]
    );
}

#[test]
fn panes_reject_bad_json() {
    assert!(parse_panes("{").is_err());
}

#[test]
fn version_parses_second_token() {
    assert_eq!(parse_version("zellij 0.44.3\n").as_deref(), Some("0.44.3"));
    assert_eq!(parse_version(""), None);
}

fn touch(path: &std::path::Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"").unwrap();
}

#[test]
fn find_prefers_explicit_then_path_then_home_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let in_path = tmp.path().join("bin").join("zellij");
    let in_cargo = home.join(".cargo").join("bin").join("zellij");
    let explicit = tmp.path().join("custom").join("zellij.exe");
    touch(&in_path);
    touch(&in_cargo);
    touch(&explicit);
    let path_env: OsString =
        std::env::join_paths([tmp.path().join("empty"), tmp.path().join("bin")]).unwrap();

    assert_eq!(
        find_zellij(Some(&explicit), Some(&path_env), &home),
        Some(explicit.clone())
    );
    assert_eq!(find_zellij(None, Some(&path_env), &home), Some(in_path));
    assert_eq!(find_zellij(None, None, &home), Some(in_cargo));
    let missing = tmp.path().join("missing");
    assert_eq!(find_zellij(Some(&missing), Some(&path_env), &home), None);
    assert_eq!(find_zellij(None, None, &tmp.path().join("nohome")), None);
}
