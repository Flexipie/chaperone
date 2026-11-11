//! `install` and `uninstall` through the binary, with `HOME` (so the data
//! dir) in a temp dir and a fake `claude` first on `PATH` that logs its
//! argv and refuses both removals, as Claude Code does when nothing is
//! installed.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

#[test]
fn install_writes_the_embedded_plugin_and_registers_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (bin, log) = (tmp.path().join("bin"), tmp.path().join("claude.log"));
    std::fs::create_dir(&bin).unwrap();
    let fake = bin.join("claude");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\necho \"$*\" >> {log:?}\ncase \"$2 $3\" in\n\
             uninstall*|\"marketplace remove\") echo not found >&2; exit 1;;\nesac\n"
        ),
    )
    .unwrap();
    let mode = std::os::unix::fs::PermissionsExt::from_mode(0o755);
    std::fs::set_permissions(&fake, mode).unwrap();
    let run = |cmd: &str| -> Value {
        let path = std::env::var("PATH").unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_chaperone"))
            .arg(cmd)
            .env("HOME", tmp.path())
            .env_remove("XDG_DATA_HOME")
            .env("PATH", format!("{}:{path}", bin.display()))
            .output()
            .unwrap();
        assert!(out.status.success(), "{cmd}: {out:?}");
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"].clone()
    };
    let dir = PathBuf::from(run("install")["dir"].as_str().unwrap());
    assert!(dir.starts_with(tmp.path()), "{dir:?}");
    let stale = dir.join("chaperone/skills/gone/SKILL.md");
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, "old").unwrap();
    let data = run("install");
    assert!(!stale.exists());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugin");
    let mut git = Command::new("git");
    let tracked = git.arg("-C").arg(&root).args(["ls-files", "-z"]).output();
    let tracked = String::from_utf8(tracked.unwrap().stdout).unwrap();
    assert!(tracked.split_terminator('\0').count() >= 2);
    for rel in tracked.split_terminator('\0').map(Path::new) {
        let (want, got) = (
            std::fs::read(root.join(rel)).unwrap(),
            std::fs::read(dir.join("chaperone").join(rel)).unwrap(),
        );
        if rel == Path::new(".claude-plugin/plugin.json") {
            let mut got: Value = serde_json::from_slice(&got).unwrap();
            let version = got.as_object_mut().unwrap().remove("version").unwrap();
            assert_eq!(version, data["version"]);
            assert!(
                version
                    .as_str()
                    .unwrap()
                    .starts_with(env!("CARGO_PKG_VERSION"))
            );
            assert_eq!(got, serde_json::from_slice::<Value>(&want).unwrap());
        } else {
            assert_eq!(got, want, "{rel:?}");
        }
    }
    let index = std::fs::read(dir.join(".claude-plugin/marketplace.json")).unwrap();
    let index: Value = serde_json::from_slice(&index).unwrap();
    assert_eq!(index["plugins"][0]["name"], "chaperone");
    assert_eq!(index["plugins"][0]["source"], "./chaperone");
    let added = format!(
        "plugin marketplace add {}\nplugin install chaperone@chaperone --scope user\n",
        dir.display()
    );
    assert_eq!(std::fs::read_to_string(&log).unwrap(), added.repeat(2));
    let data = run("uninstall");
    assert_eq!(data["ran"][0]["exit"], 1);
    assert_eq!(data["ran"][1]["exit"], 1);
    assert!(!dir.exists());
    assert!(!dir.parent().unwrap().join("ledger.db").exists());
    let removed = "plugin uninstall chaperone@chaperone --scope user\n\
                   plugin marketplace remove chaperone\n";
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        added.repeat(2) + removed
    );
}
