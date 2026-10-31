#![cfg(target_os = "linux")]
use std::{
    ffi::OsString,
    fs,
    os::unix::{ffi::OsStringExt, process::CommandExt},
    path::Path,
    process::Command,
};
use zeron_mcp_proxy::shim;

#[test]
fn shell_passthrough_preserves_argv0_non_utf8_env_fds_and_exit_code_even_with_broken_manifest() {
    let root = tempfile::tempdir_in(std::env::var_os("TMPDIR").unwrap()).unwrap();
    let shims = root.path().join("shim");
    let real = root.path().join("real");
    fs::create_dir(&shims).unwrap();
    fs::create_dir(&real).unwrap();
    let helper = env!("CARGO_BIN_EXE_zeron-mcp-proxy");
    let fake = env!("CARGO_BIN_EXE_fake-mcp");
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(&shims, &alias).unwrap();
    let path =
        std::env::join_paths([shims.clone(), alias, real.clone(), "/usr/bin".into()]).unwrap();
    fs::write(shims.join(shim::MANIFEST), "corrupt").unwrap();
    for command in ["node", "npx", "python3"] {
        std::os::unix::fs::symlink(helper, shims.join(command)).unwrap();
        std::os::unix::fs::symlink(fake, real.join(command)).unwrap();
        for manifest in [true, false] {
            let run = |program: &Path| {
                let mut cmd = Command::new(program);
                cmd.arg0("custom-argv0")
                    .arg(OsString::from_vec(vec![b'x', 0xff]))
                    .env("PATH", &path)
                    .env("FAKE_EXEC_PROBE", "1")
                    .env("SHIM_SENTINEL", "exact value")
                    .env(shim::DIR_ENV, &shims);
                if manifest {
                    cmd.env(shim::PARENT_ENV, std::env::current_exe().unwrap());
                } else {
                    cmd.env_remove(shim::DIR_ENV).env_remove(shim::PARENT_ENV);
                }
                unsafe {
                    cmd.pre_exec(|| {
                        let fd = libc::open(c"/dev/zero".as_ptr(), libc::O_RDONLY);
                        if fd < 0 || libc::dup2(fd, 9) < 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                        if fd != 9 {
                            libc::close(fd);
                        }
                        Ok(())
                    });
                }
                cmd.output().unwrap()
            };
            let baseline = run(&real.join(command));
            let shimmed = run(&shims.join(command));
            assert_eq!(baseline.status.code(), Some(37));
            assert_eq!(
                shimmed.status.code(),
                Some(37),
                "{}",
                String::from_utf8_lossy(&shimmed.stderr)
            );
            assert_eq!(baseline.stdout, shimmed.stdout);
            let record: serde_json::Value = serde_json::from_slice(&shimmed.stdout).unwrap();
            assert_eq!(record["fd9"], true);
        }
    }
}

#[test]
fn bash_custom_argv0_exec_passes_through_even_when_server_args_match() {
    let root = tempfile::tempdir_in(std::env::var_os("TMPDIR").unwrap()).unwrap();
    let shims = root.path().join("shim");
    let real = root.path().join("real");
    fs::create_dir(&shims).unwrap();
    fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_zeron-mcp-proxy"), shims.join("node")).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_fake-mcp"), real.join("node")).unwrap();
    let parent = std::env::current_exe().unwrap();
    let launch = shim::Launch {
        version: 1,
        key: "fresh".into(),
        parent_exe: parent.clone(),
        parent_identity: zeron_mcp_proxy::cache::binary_id(&parent),
        data_dir: root.path().into(),
        run_dir: root.path().join("run"),
        idle_minutes: 10,
        servers: vec![shim::Server {
            name: "exact-match".into(),
            command: "node".into(),
            resolved_command: real.join("node").canonicalize().unwrap(),
            args: vec!["argument".into()],
            mode: "lazy".into(),
            channel: false,
            owner_legacy: false,
        }],
    };
    fs::write(
        shims.join(shim::MANIFEST),
        serde_json::to_vec(&launch).unwrap(),
    )
    .unwrap();
    let output = Command::new("/bin/bash")
        .args(["-c", "exec -a custom-node node argument"])
        .env("PATH", std::env::join_paths([shims.clone(), real]).unwrap())
        .env(shim::DIR_ENV, &shims)
        .env(shim::KEY_ENV, "fresh")
        .env(shim::PARENT_ENV, &parent)
        .env("SHLVL", "1")
        .env("FAKE_EXEC_PROBE", "1")
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(37),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        record["args"][0],
        serde_json::json!(b"custom-node".to_vec())
    );
}
