use crate::{
    Arch,
    ensure_dir,
    ensure_dir_clean,
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
    dist: &'a str,
    version: &'a str,
    dir: &'a Path,
}

#[derive(Deserialize, Serialize)]
pub struct Pkg {
    pub name: String,
    #[serde(default)]
    pub patches: Vec<String>,
}

fn source_values(source: &str, key: &str) -> io::Result<Vec<String>> {
    let mut values = Vec::new();

    let key_sep = format!("{}: ", key);
    for line in source.lines() {
        if line.starts_with(&key_sep) {
            values.push(line[key_sep.len()..].to_string());
        }
    }

    if ! values.is_empty() {
        Ok(values)
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("failed to find '{}' key in source", key)
        ))
    }
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

    fn source(&self, config: &Config) -> io::Result<PathBuf> {
        let dir = config.dir.join("source");
        if dir.is_dir() {
            return Ok(dir);
        }

        let share_name = format!("popopt_{}_{}_{}_{}", config.arch.name, config.dist, self.name, config.version);
        let share_dir = ensure_dir_clean(format!("/var/lib/sbuild/build/{}", share_name))?;

        // Download package source
        //TODO: Add updates and security
        process::Command::new("schroot")
            //TODO: Use sbuild arch?
            .arg("--chroot").arg(format!("{}-amd64-popopt", config.dist))
            .arg("--directory").arg(format!("/build/{}", share_name))
            .arg("--")
            .arg("apt-get")
            .arg("source")
            .arg("--only-source")
            .arg("--download-only")
            .arg(format!("{}={}", self.name, config.version))
            .current_dir(&config.dir)
            .status()
            .and_then(status_err)?;

        let dsc_file = share_dir.join(format!("{}_{}.dsc", self.name, config.version));
        if ! dsc_file.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("failed to find DSC file '{}'", dsc_file.display())
            ));
        }

        // Extract package source
        process::Command::new("dpkg-source")
            .arg("--extract")
            .arg(&dsc_file)
            .arg(&dir)
            .current_dir(&config.dir)
            .status()
            .and_then(status_err)?;

        fs::remove_dir_all(&share_dir)?;

        Ok(dir)
    }

    fn patched(&self, source_dir: &Path, config: &Config) -> io::Result<PathBuf> {
        let dir = config.dir.join("patched");
        if dir.is_dir() {
            fs::remove_dir_all(&dir)?;
        }

        process::Command::new("cp")
            .arg("-a")
            .arg(&source_dir)
            .arg(&dir)
            .current_dir(&config.dir)
            .status()
            .and_then(status_err)?;

        // Apply additional source patches
        for patch in self.patches.iter() {
            let patch_file = fs::canonicalize(patch)?;
            process::Command::new("patch")
                .arg("-p1")
                .arg("-i").arg(&patch_file)
                .current_dir(&dir)
                .status()
                .and_then(status_err)?;
        }

        // Update changelog
        process::Command::new("dch")
            .arg("--distribution").arg(config.dist)
            .arg("--newversion").arg(format!("{}popopt{}", config.version, config.arch.level))
            .arg("Pop!_OS Optimizations")
            .current_dir(&dir)
            .status()
            .and_then(status_err)?;

        Ok(dir)
    }

    fn sbuild(&self, source_dir: &Path, sbuild_arch: &str, config: &Config) -> io::Result<PathBuf> {
        let dir = config.dir.join(format!("sbuild-{}", sbuild_arch));
        if dir.is_dir() {
            //TODO: rebuild
            //fs::remove_dir_all(&dir)?;
            return Ok(dir);
        }
        fs::create_dir(&dir)?;

        // Create sbuild config
        //TODO: can flags be passed as an array?
        let sbuild_conf = format!(
r#"$build_environment = {{
    'DEB_CFLAGS_APPEND' => '{}',
    'DEB_CXXFLAGS_APPEND' => '{}',
    'POP_OPT_ARCH' => '{}',
    'RUSTFLAGS' => '{}',
}};
"#,
            config.arch.cflags().join(" "),
            config.arch.cxxflags().join(" "),
            config.arch.name,
            config.arch.rustflags().join(" "),
        );
        let sbuild_conf_file = dir.join("sbuild.conf");
        fs::write(&sbuild_conf_file, sbuild_conf)?;

        let mut command = process::Command::new("sbuild");
        if sbuild_arch == "amd64" {
            command.arg("--arch-all");
        } else {
            command.arg("--no-arch-all");
        }
        command
            .arg("--no-apt-distupgrade")
            .arg("--verbose")
            .arg(format!("--chroot={}-{}-popopt", config.dist, sbuild_arch))
            .arg(format!("--dist={}", config.dist))
            .arg(format!("--arch={}", sbuild_arch))
            .arg(format!("--extra-repository=deb http://us.archive.ubuntu.com/ubuntu/ {}-updates main restricted universe multiverse", config.dist))
            .arg(format!("--extra-repository=deb http://us.archive.ubuntu.com/ubuntu/ {}-security main restricted universe multiverse", config.dist))
            .arg(&source_dir)
            .current_dir(&dir)
            .env("SBUILD_CONFIG", &sbuild_conf_file)
            .status()
            .and_then(status_err)?;

        Ok(dir)
    }

    pub fn build<P: AsRef<Path>>(&self, arch: &Arch, dist: &str, sbuild_archs: &[&str], dir: P) -> io::Result<Vec<PathBuf>> {
        let dir = dir.as_ref();

        println!("- Package {} in {}", self.name, dir.display());

        // Get version of source
        let output = process::Command::new("schroot")
            //TODO: Use sbuild arch?
            .arg("--chroot").arg(format!("{}-amd64-popopt", dist))
            .arg("--directory").arg("/root")
            .arg("--user").arg("root")
            .arg("--")
            .arg("apt-cache")
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

        let packages = source_values(source, "Package")?;
        for package in packages.iter() {
            if &self.name != package {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("requested source '{}' does not match source '{}'", self.name, package)
                ));
            }
        }

        let versions = source_values(source, "Version")?;
        let mut version = &versions[0];
        for other_version in versions.iter() {
            let status = process::Command::new("dpkg")
                .arg("--compare-versions")
                .arg(other_version)
                .arg("gt")
                .arg(version)
                .status()?;
            match status.code() {
                Some(0) => version = other_version,
                _ => (),
            }
        }

        let version_dir = ensure_dir(dir.join(&version))?;
        println!("  - Version {} in {}", version, version_dir.display());

        let config = Config {
            arch,
            dist,
            version: &version,
            dir: &version_dir,
        };

        let source_dir = self.source(&config)?;
        let patched_dir = self.patched(&source_dir, &config)?;

        let mut debs = Vec::new();
        for sbuild_arch in sbuild_archs {
            println!("    - sbuild {}", sbuild_arch);
            let sbuild_dir = self.sbuild(&patched_dir, sbuild_arch, &config)?;
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
