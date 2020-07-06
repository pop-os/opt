use crate::{
    Arch,
    ensure_dir,
    status_err,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io,
    path::{Path, PathBuf},
    process,
    str,
};

struct Config<'a> {
    arch: &'a Arch,
    dir: &'a Path,
}

#[derive(Deserialize, Serialize)]
pub struct Pkg {
    pub name: String,
    #[serde(default)]
    pub patches: Vec<String>,
}

fn source_value(source: &str, key: &str) -> io::Result<String> {
    let key_sep = format!("{}: ", key);
    for line in source.lines() {
        if line.starts_with(&key_sep) {
            return Ok(line[key_sep.len()..].to_string())
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("failed to find '{}' key in source", key)
    ))
}

impl Pkg {
    pub fn load<P: AsRef<Path>>(p: P) -> io::Result<Self> {
        let data = fs::read_to_string(p)?;
        toml::from_str(&data).map_err(|err| io::Error::new(
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

    fn source(&self, version: &str, config: &Config) -> io::Result<PathBuf> {
        // Download package source
        process::Command::new("apt-get")
            .arg("source")
            .arg("--only-source")
            .arg("--download-only")
            .arg(format!("{}={}", self.name, version))
            .current_dir(&config.dir)
            .status()
            .and_then(status_err)?;

        let dsc_file = config.dir.join(format!("{}_{}.dsc", self.name, version));
        if ! dsc_file.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("failed to find DSC file '{}'", dsc_file.display())
            ));
        }

        // Extract package source
        let source_dir = config.dir.join("source");
        if ! source_dir.is_dir() {
            process::Command::new("dpkg-source")
                .arg("--extract")
                .arg(&dsc_file)
                .arg(&source_dir)
                .current_dir(&config.dir)
                .status()
                .and_then(status_err)?;
        }

        Ok(source_dir)
    }

    fn patched(&self, source_dir: &Path, config: &Config) -> io::Result<PathBuf> {
        // Apply additional source patches
        let patched_dir = config.dir.join("patched");
        if patched_dir.is_dir() {
            fs::remove_dir_all(&patched_dir)?;
        }
        process::Command::new("cp")
            .arg("-a")
            .arg(&source_dir)
            .arg(&patched_dir)
            .current_dir(&config.dir)
            .status()
            .and_then(status_err)?;
        for patch in self.patches.iter() {
            let patch_file = fs::canonicalize(patch)?;
            process::Command::new("patch")
                .arg("-p1")
                .arg("-i").arg(&patch_file)
                .current_dir(&patched_dir)
                .status()
                .and_then(status_err)?;
        }

        Ok(patched_dir)
    }

    fn sbuild(&self, source_dir: &Path, sbuild_dist: &str, sbuild_arch: &str, config: &Config) -> io::Result<PathBuf> {
        let sbuild_dir = config.dir.join(format!("sbuild-{}", sbuild_arch));
        if sbuild_dir.is_dir() {
            //TODO: rebuild
            //fs::remove_dir_all(&sbuild_dir)?;
            return Ok(sbuild_dir);
        }
        fs::create_dir(&sbuild_dir)?;

        // Create sbuild config
        //TODO: can flags be passed as an array?
        let sbuild_conf = format!(
r#"$build_environment = {{
    'DEB_CFLAGS_APPEND' => '{}',
    'DEB_CXXFLAGS_APPEND' => '{}',
    'RUSTFLAGS' => '{}',
}};
"#,
            config.arch.cflags().join(" "),
            config.arch.cxxflags().join(" "),
            config.arch.rustflags().join(" "),
        );
        let sbuild_conf_file = sbuild_dir.join("sbuild.conf");
        fs::write(&sbuild_conf_file, sbuild_conf)?;

        process::Command::new("sbuild")
            .arg("--no-apt-distupgrade")
            .arg(format!("--dist={}", sbuild_dist))
            .arg(format!("--arch={}", sbuild_arch))
            .arg(format!("--extra-repository=deb http://us.archive.ubuntu.com/ubuntu/ {}-updates main restricted universe multiverse", sbuild_dist))
            .arg(format!("--extra-repository=deb http://us.archive.ubuntu.com/ubuntu/ {}-security main restricted universe multiverse", sbuild_dist))
            .arg(&source_dir)
            .current_dir(&sbuild_dir)
            .env("SBUILD_CONFIG", &sbuild_conf_file)
            .status()
            .and_then(status_err)?;

        Ok(sbuild_dir)
    }

    pub fn build<P: AsRef<Path>>(&self, arch: &Arch, sbuild_dist: &str, sbuild_archs: &[&str], dir: P) -> io::Result<Vec<PathBuf>> {
        let dir = dir.as_ref();

        println!("- Package {} in {}", self.name, dir.display());

        //TODO: allow downloading for other series
        let output = process::Command::new("apt-cache")
            .arg("showsrc")
            .arg("--only-source")
            .arg(&self.name)
            .current_dir(&dir)
            .stdout(process::Stdio::piped())
            .spawn()?
            .wait_with_output()?;
        status_err(output.status)?;
        let source = str::from_utf8(&output.stdout).map_err(|err| io::Error::new(
            io::ErrorKind::InvalidData,
            err
        ))?;

        let package = source_value(source, "Package")?;
        if self.name != package {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("requested source '{}' does not match source '{}'", self.name, package)
            ));
        }

        let version = source_value(source, "Version")?;
        let version_dir = ensure_dir(dir.join(&version))?;
        println!("  - Version {} in {}", version, version_dir.display());

        let config = Config {
            arch,
            dir: &version_dir,
        };

        let source_dir = self.source(&version, &config)?;
        let patched_dir = self.patched(&source_dir, &config)?;

        let mut debs = Vec::new();
        for sbuild_arch in sbuild_archs {
            println!("    - sbuild {}", sbuild_arch);
            let sbuild_dir = self.sbuild(&patched_dir, sbuild_dist, sbuild_arch, &config)?;
            for entry_res in fs::read_dir(&sbuild_dir)? {
                let entry = entry_res?;
                if entry.file_name().to_str().unwrap_or("").ends_with(".deb") {
                    debs.push(entry.path());
                }
            }
        }

        Ok(debs)
    }
}
