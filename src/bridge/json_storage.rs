
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs::{self, create_dir_all, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub fn get_config_path(filename: &str) -> io::Result<PathBuf> {
    let path = app_dir()?.join(filename);
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    Ok(path)
}


#[cfg(not(test))]
fn app_dir() -> io::Result<PathBuf> {
    crate::app()
        .map(|app| app.data_dir.clone())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "The app has not registered its storage directory",
            )
        })
}


#[cfg(test)]
fn app_dir() -> io::Result<PathBuf> {
    Ok(std::env::temp_dir().join(format!("dannesk-test-{}", std::process::id())))
}


pub fn tmp_name(filename: &str) -> String {
    format!("{filename}.tmp")
}

pub fn write_json<T: Serialize>(filename: &str, data: &T) -> io::Result<()> {
    let json = serde_json::to_string(data)?;
    write_bytes(filename, json.as_bytes())
}


pub fn write_bytes(filename: &str, bytes: &[u8]) -> io::Result<()> {
    let path = get_config_path(filename)?;
    let tmp = get_config_path(&tmp_name(filename))?;

    write_through_tmp(&tmp, &path, bytes).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

fn write_through_tmp(tmp: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
 
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }

    let mut file = opts.open(tmp)?;

 
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }

    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);

    fs::rename(tmp, path)?;

  
    #[cfg(unix)]
    if let Some(dir) = path.parent()
        && let Ok(handle) = File::open(dir)
    {
        let _ = handle.sync_all();
    }

    Ok(())
}

pub fn read_json<T: DeserializeOwned>(filename: &str) -> io::Result<T> {
    let path = get_config_path(filename)?;
    let content = fs::read_to_string(path)?;
    let data = serde_json::from_str(&content)?;
    Ok(data)
}


pub fn read_bytes(filename: &str) -> io::Result<Option<Vec<u8>>> {
    let path = get_config_path(filename)?;
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn update_json<T: Serialize + DeserializeOwned + std::default::Default>(
    filename: &str,
    update_fn: impl FnOnce(&mut T),
) -> io::Result<()> {

    let mut data = match read_json(filename) {
        Ok(data) => data,
        Err(e) if e.kind() == io::ErrorKind::NotFound => T::default(),
        Err(e) => return Err(e),
    };
    update_fn(&mut data);
    write_json(filename, &data)?;
    Ok(())
}

pub fn remove_json(filename: &str) -> io::Result<()> {
    let path = get_config_path(filename)?;
    let tmp = get_config_path(&tmp_name(filename))?;

    let removed = match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    };

    match fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
      
        Err(e) => return Err(e),
    }

    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

 
    #[test]
    fn a_write_leaves_no_tmp_behind() {
        let name = "test_atomic_write.json";
        let _ = remove_json(name);

        write_json(name, &json!({ "address": "rTest" })).expect("first write");
        write_json(name, &json!({ "address": "rSecond" })).expect("second write");

        let read: serde_json::Value = read_json(name).expect("read back");
        assert_eq!(read["address"], "rSecond");
        assert!(
            !get_config_path(&tmp_name(name)).unwrap().exists(),
            "a completed write must not leave its tmp on disk",
        );

        let _ = remove_json(name);
    }


    #[test]
    fn update_refuses_to_default_over_an_unparseable_file() {
        let name = "test_update_garbage.json";
        write_bytes(name, b"{ this is not json").expect("seed garbage");

        let result = update_json(name, |data: &mut serde_json::Value| {
            if let Some(obj) = data.as_object_mut() {
                obj.insert("touched".to_string(), json!(true));
            }
        });

        assert!(result.is_err(), "a corrupt file must not be overwritten");
        assert_eq!(
            read_bytes(name).unwrap().as_deref(),
            Some(&b"{ this is not json"[..]),
            "the original bytes must survive the failed update",
        );

        let _ = remove_json(name);
    }


    #[test]
    fn update_still_creates_a_missing_file() {
        let name = "test_update_missing.json";
        let _ = remove_json(name);

        update_json(name, |data: &mut serde_json::Value| {
            *data = json!({ "created": true });
        })
        .expect("missing file is the create case");

        let read: serde_json::Value = read_json(name).expect("read back");
        assert_eq!(read["created"], true);

        let _ = remove_json(name);
    }

    #[test]
    fn remove_is_idempotent_and_takes_the_tmp() {
        let name = "test_remove_tmp.json";
        write_json(name, &json!({ "address": "rTest" })).expect("write");
        fs::write(get_config_path(&tmp_name(name)).unwrap(), b"leftover").expect("plant a tmp");

        remove_json(name).expect("first removal");
        assert!(!get_config_path(name).unwrap().exists());
        assert!(!get_config_path(&tmp_name(name)).unwrap().exists());

        remove_json(name).expect("removing what is already gone is a success");
    }
}
