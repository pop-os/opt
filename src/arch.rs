use serde::{Deserialize, Serialize};
use std::{
    fs,
    io,
    path::Path,
    process,
    str,
};

#[derive(Deserialize, Serialize)]
pub struct Arch {
    pub name: String,
    pub wiki: String,
    pub features: Vec<String>,
}

impl Arch {
    pub fn load<P: AsRef<Path>>(p: P) -> io::Result<Self> {
        let json = fs::read_to_string(p)?;
        serde_json::from_str(&json).map_err(|err| io::Error::new(
            io::ErrorKind::InvalidData,
            err,
        ))
    }

    pub fn load_all<P: AsRef<Path>>(p: P) -> io::Result<Vec<Self>> {
        let mut entries = Vec::new();
        for entry_res in fs::read_dir(p)? {
            entries.push(entry_res?.path());
        }
        entries.sort();

        let mut archs = Vec::new();
        for entry in entries.iter() {
            archs.push(Self::load(entry)?);
        }
        Ok(archs)
    }

    pub fn cflags(&self) -> Vec<String> {
        vec![
            format!("-march={}", self.name),
        ]
    }

    pub fn rustflags(&self) -> Vec<String> {
        vec![
            format!("--codegen"),
            format!("target-cpu={}", self.name),
        ]
    }

    pub fn cpu_features() -> io::Result<Vec<String>> {
        //TODO: smarter check for features
        let output = process::Command::new("bash")
            .arg("-c")
            .arg("grep '^flags' /proc/cpuinfo | head -n 1 | sed 's/^flags.*: //'")
            .output()?;
        let stdout = str::from_utf8(&output.stdout).map_err(|err| io::Error::new(
            io::ErrorKind::InvalidData,
            err,
        ))?;

        Ok(
            stdout.split(' ')
                .map(|x| x.trim().to_string())
                .collect()
        )
    }

    pub fn check_features(&self, cpu_features: &[String]) -> Result<(), Vec<String>> {
        let mut missing = self.features.clone();
        missing.retain(|x| !cpu_features.contains(x));
        if missing.is_empty() {
            Ok(())
        } else {
            Err(missing)
        }
    }
}
