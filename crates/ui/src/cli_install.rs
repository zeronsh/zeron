use std::path::Path;
use std::process::Command;

use zeron_update::InstallKind;

const BIN_DIR: &str = "/usr/local/bin";
const LINK: &str = "/usr/local/bin/zeron";

pub(crate) fn install() -> Result<String, String> {
    let InstallKind::MacApp { bundle } = zeron_update::detect_install() else {
        return Err("Run the installed Zeron.app to install the zeron command".into());
    };
    let exe = bundle.join("Contents/MacOS/zeron");
    let link = Path::new(LINK);
    if std::fs::read_link(link).is_ok_and(|target| target == exe) {
        return Ok(format!("{LINK} already points at this Zeron"));
    }
    match link_in_place(&exe, link) {
        Ok(()) => Ok(format!("Installed {LINK}")),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => link_as_admin(&exe),
        Err(error) => Err(format!("Could not install {LINK}: {error}")),
    }
}

fn link_in_place(exe: &Path, link: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(BIN_DIR)?;
    match std::fs::remove_file(link) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    std::os::unix::fs::symlink(exe, link)
}

fn link_as_admin(exe: &Path) -> Result<String, String> {
    let exe = exe.to_string_lossy().replace('\'', "'\\''");
    let shell = format!("mkdir -p {BIN_DIR} && ln -sf '{exe}' {LINK}");
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        shell.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let status = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .status()
        .map_err(|error| format!("Could not run osascript: {error}"))?;
    if status.success() {
        Ok(format!("Installed {LINK}"))
    } else {
        Err("Installing the zeron command was cancelled".into())
    }
}
