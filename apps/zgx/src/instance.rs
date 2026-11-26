use std::collections::HashMap;
use std::error::Error;
use std::net::TcpListener;
use std::path::{Path, PathBuf};

const ISOLATED: &str = "ZERON_ISOLATED";
const INSTANCE_ID: &str = "ZERON_INSTANCE_ID";
const DATA_DIR: &str = "ZERON_DATA_DIR";
const IPC_PORT: &str = "ZERON_IPC_PORT";
const ENGINE_DATA: &str = "ZERON_ZGX_ENGINE_DATA";
const ENGINE_PORT: &str = "ZERON_ZGX_ENGINE_PORT";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Settings {
    id: String,
    data_dir: PathBuf,
    port: u16,
}

pub fn initialize() -> Result<(), Box<dyn Error>> {
    let isolated = std::env::var(ISOLATED).as_deref() == Ok("1");
    let has_id = std::env::var_os(INSTANCE_ID).is_some();
    if !isolated && !has_id {
        return Ok(());
    }

    let mut vars = HashMap::new();
    for key in [INSTANCE_ID, DATA_DIR, IPC_PORT, ENGINE_DATA, ENGINE_PORT] {
        if let Some(value) = std::env::var_os(key) {
            let value = value.into_string().map_err(|_| format!("{key} must be valid Unicode"))?;
            vars.insert(key.to_owned(), value);
        }
    }
    let instances_root = match vars
        .get(INSTANCE_ID)
        .and_then(|_| vars.get(DATA_DIR))
        .and_then(|dir| Path::new(dir).parent())
        .filter(|dir| dir.file_name().is_some_and(|name| name == ".zeron-instances"))
        .map(Path::to_path_buf)
    {
        Some(root) => root,
        None => instances_root()?,
    };

    let settings = match settings_from(&vars, &instances_root, None)? {
        Some(settings) => settings,
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            let port = choose_port()?;
            let settings = Settings {
                data_dir: instances_root.join(&id),
                id,
                port,
            };
            settings_from(&vars, &instances_root, Some(settings))?.ok_or("missing isolated settings")?
        }
    };

    if has_id {
        if !settings.data_dir.is_dir() {
            return Err("isolated instance data directory does not exist".into());
        }
        validate_manifest(&settings)?;
        return Ok(());
    }

    let data_dir = settings.data_dir.to_str().ok_or("isolated data directory must be valid Unicode")?;
    std::fs::create_dir_all(&instances_root)?;
    std::fs::create_dir(&settings.data_dir)?;
    write_manifest(&settings)?;

    set_env(INSTANCE_ID, &settings.id);
    set_path_env(DATA_DIR, &settings.data_dir);
    set_env(IPC_PORT, &settings.port.to_string());
    set_path_env(ENGINE_DATA, &settings.data_dir);
    set_env(ENGINE_PORT, &settings.port.to_string());
    unsafe {
        std::env::remove_var("ZERON_IPC_TOKEN");
        std::env::remove_var("ZERON_IPC_TOKEN_FILE");
    }
    let data_dir = shell_quote(data_dir);
    eprintln!("zeron-zgx: isolated Zeron instance. Copy these values into a POSIX shell:\nexport ZERON_ISOLATED=1\nexport ZERON_INSTANCE_ID={}\nexport ZERON_DATA_DIR={}\nexport ZERON_IPC_PORT={}\nexport ZERON_ZGX_ENGINE_DATA={}\nexport ZERON_ZGX_ENGINE_PORT={}", settings.id, data_dir, settings.port, data_dir, settings.port);
    Ok(())
}

fn set_env(key: &str, value: &str) {
    unsafe { std::env::set_var(key, value) }
}

fn set_path_env(key: &str, value: &Path) {
    unsafe { std::env::set_var(key, value) }
}

fn instances_root() -> Result<PathBuf, Box<dyn Error>> {
    #[cfg(windows)]
    let root = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join("AppData").join("Local"))
        })
        .ok_or("LOCALAPPDATA and USERPROFILE are not set")?;
    #[cfg(not(windows))]
    let root = std::env::var_os("HOME").map(PathBuf::from).ok_or("HOME is not set")?;
    Ok(root.join(".zeron-instances"))
}

fn choose_port() -> Result<u16, Box<dyn Error>> {
    loop {
        let port = TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port();
        if !matches!(port, 27654 | 27655 | 27871) {
            return Ok(port);
        }
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn manifest_path(settings: &Settings) -> PathBuf {
    settings.data_dir.join("instance.json")
}

fn write_manifest(settings: &Settings) -> Result<(), Box<dyn Error>> {
    let bytes = serde_json::to_vec(&serde_json::json!({"id": settings.id.as_str(), "port": settings.port}))?;
    std::fs::write(manifest_path(settings), bytes)?;
    Ok(())
}

fn validate_manifest(settings: &Settings) -> Result<(), Box<dyn Error>> {
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(manifest_path(settings))?)?;
    if value["id"].as_str() != Some(settings.id.as_str()) || value["port"].as_u64() != Some(u64::from(settings.port)) {
        return Err("isolated instance manifest conflicts with environment".into());
    }
    Ok(())
}

fn settings_from(
    vars: &HashMap<String, String>,
    instances_root: &Path,
    fresh: Option<Settings>,
) -> Result<Option<Settings>, Box<dyn Error>> {
    let Some(id) = vars.get(INSTANCE_ID) else {
        if [DATA_DIR, IPC_PORT, ENGINE_DATA, ENGINE_PORT]
            .iter()
            .any(|key| vars.contains_key(*key))
        {
            return Err("isolated configuration is incomplete without ZERON_INSTANCE_ID".into());
        }
        return Ok(fresh);
    };

    let parsed = uuid::Uuid::parse_str(id)?;
    if parsed.to_string() != *id {
        return Err("ZERON_INSTANCE_ID must be a canonical UUID".into());
    }
    let expected_dir = instances_root.join(id);
    let data_dir = required(vars, DATA_DIR)?;
    let engine_data = required(vars, ENGINE_DATA)?;
    let port = required(vars, IPC_PORT)?.parse::<u16>()?;
    let engine_port = required(vars, ENGINE_PORT)?.parse::<u16>()?;
    if port == 0 || engine_port == 0 {
        return Err("isolated IPC ports must be nonzero".into());
    }
    if Path::new(data_dir) != expected_dir || Path::new(engine_data) != expected_dir || port != engine_port {
        return Err("isolated configuration conflicts with ZERON_INSTANCE_ID".into());
    }
    let settings = Settings { id: id.clone(), data_dir: expected_dir, port };
    if fresh.is_some_and(|fresh| fresh != settings) {
        return Err("isolated configuration conflicts with generated settings".into());
    }
    Ok(Some(settings))
}

fn required<'a>(vars: &'a HashMap<String, String>, key: &str) -> Result<&'a str, Box<dyn Error>> {
    vars.get(key)
        .map(String::as_str)
        .ok_or_else(|| format!("isolated configuration is missing {key}").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(id: &str, root: &Path, port: &str) -> HashMap<String, String> {
        let dir = root.join(id).display().to_string();
        HashMap::from([
            (INSTANCE_ID.into(), id.into()),
            (DATA_DIR.into(), dir.clone()),
            (IPC_PORT.into(), port.into()),
            (ENGINE_DATA.into(), dir),
            (ENGINE_PORT.into(), port.into()),
        ])
    }

    #[test]
    fn accepts_complete_relaunch_settings() {
        let root = Path::new("/tmp/zeron-test");
        let id = "123e4567-e89b-42d3-a456-426614174000";
        let root = root.join(".zeron-instances");
        let settings = settings_from(&base(id, &root, "29001"), &root, None).unwrap().unwrap();
        assert_eq!(settings.id, id);
        assert_eq!(settings.port, 29001);
        assert_eq!(settings.data_dir, root.join(id));
    }

    #[test]
    fn rejects_incomplete_and_conflicting_values() {
        let root = Path::new("/tmp/zeron-test/.zeron-instances");
        let id = "123e4567-e89b-42d3-a456-426614174000";
        let mut incomplete = HashMap::new();
        incomplete.insert(INSTANCE_ID.into(), id.into());
        assert!(settings_from(&incomplete, root, None).is_err());

        let mut conflict = base(id, root, "29001");
        conflict.insert(ENGINE_PORT.into(), "29002".into());
        assert!(settings_from(&conflict, root, None).is_err());
    }

    #[test]
    fn creates_settings_from_fresh_values_without_process_environment() {
        let root = Path::new("/tmp/zeron-test/.zeron-instances");
        let id = "123e4567-e89b-42d3-a456-426614174000";
        let fresh = Settings { id: id.into(), data_dir: root.join(id), port: 29001 };
        assert_eq!(settings_from(&HashMap::new(), root, Some(fresh.clone())).unwrap(), Some(fresh));
        assert!(settings_from(&HashMap::from([(DATA_DIR.into(), "/tmp/old".into())]), root, None).is_err());
    }

    #[test]
    fn instance_manifest_round_trips_and_rejects_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            id: "123e4567-e89b-42d3-a456-426614174000".into(),
            data_dir: dir.path().to_path_buf(),
            port: 29001,
        };
        write_manifest(&settings).unwrap();
        validate_manifest(&settings).unwrap();

        let wrong = Settings { port: 29002, ..settings };
        assert!(validate_manifest(&wrong).is_err());
    }
}
