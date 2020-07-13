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
    thread,
};

struct Config<'a> {
    arch: &'a Arch,
    dist: &'a str,
    version: &'a str,
    dir: &'a Path,
    rebuild: bool,
    retry: bool,
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
        let complete_dir = config.dir.join("source");
        let new_version = format!("{}popopt{}", config.version, config.arch.level);
        let new_dsc_file = complete_dir.join(format!("{}_{}.dsc", self.name, new_version));
        if complete_dir.is_dir() {
            if config.rebuild {
                fs::remove_dir_all(&complete_dir)?;
            } else if new_dsc_file.is_file() {
                return Ok(new_dsc_file);
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("failed to find DSC file '{}'", new_dsc_file.display())
                ));
            }
        }

        let dir = config.dir.join("source.partial");
        if dir.is_dir() {
            if config.retry {
                fs::remove_dir_all(&dir)?;
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "'{}' already exists, build is in progress or already failed",
                        dir.display()
                    )
                ));
            }
        }

        fs::create_dir(&dir)?;

        let share_name = format!("popopt_{}_{}_{}_{}", config.arch.name, config.dist, self.name, config.version);
        let share_dir = ensure_dir_clean(format!("/var/lib/sbuild/build/{}", share_name))?;

        // Download package source
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
        let original_dir = dir.join("original");
        process::Command::new("dpkg-source")
            .arg("--extract")
            .arg(&dsc_file)
            .arg(&original_dir)
            .current_dir(&dir)
            .status()
            .and_then(status_err)?;

        fs::remove_dir_all(&share_dir)?;

        // Make a copy where patches are applied
        let patched_dir = dir.join("patched");
        process::Command::new("cp")
            .arg("-a")
            .arg(&original_dir)
            .arg(&patched_dir)
            .current_dir(&dir)
            .status()
            .and_then(status_err)?;

        // Apply additional source patches
        for patch in self.patches.iter() {
            let patch_file = fs::canonicalize(patch)?;
            process::Command::new("patch")
                .arg("-p1")
                .arg("-i").arg(&patch_file)
                .current_dir(&patched_dir)
                .status()
                .and_then(status_err)?;
        }

        // Update changelog
        process::Command::new("dch")
            .arg("--distribution").arg(config.dist)
            .arg("--newversion").arg(&new_version)
            .arg("Pop!_OS Optimizations")
            .current_dir(&patched_dir)
            .status()
            .and_then(status_err)?;

        // Create DSC file
        process::Command::new("dpkg-source")
            .arg("--build").arg(&patched_dir)
            .current_dir(&dir)
            .status()
            .and_then(status_err)?;

        fs::rename(&dir, &complete_dir)?;

        if ! new_dsc_file.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("failed to find DSC file '{}'", new_dsc_file.display())
            ));
        }

        Ok(new_dsc_file)
    }

    fn sbuild_thread(&self, source_dsc: &Path, sbuild_arch: &str, config: &Config) -> io::Result<thread::JoinHandle<io::Result<PathBuf>>> {
        let complete_dir = config.dir.join(format!("sbuild-{}", sbuild_arch));
        if complete_dir.is_dir() {
            if config.rebuild {
                fs::remove_dir_all(&complete_dir)?;
            } else {
                return Ok(thread::spawn(move || {
                    Ok(complete_dir)
                }));
            }
        }

        let dir = config.dir.join(format!("sbuild-{}.partial", sbuild_arch));
        if dir.is_dir() {
            if config.retry {
                fs::remove_dir_all(&dir)?;
            } else {
                return Ok(thread::spawn(move || {
                    Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!(
                            "'{}' already exists, build is in progress or already failed",
                            dir.display()
                        )
                    ))
                }));
            }
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
            .arg("--quiet")
            .arg(format!("--chroot={}-{}-popopt", config.dist, sbuild_arch))
            .arg(format!("--dist={}", config.dist))
            .arg(format!("--arch={}", sbuild_arch))
            .arg(format!("--extra-repository=deb http://us.archive.ubuntu.com/ubuntu/ {}-updates main restricted universe multiverse", config.dist))
            .arg(format!("--extra-repository=deb http://us.archive.ubuntu.com/ubuntu/ {}-security main restricted universe multiverse", config.dist))
            .arg(&source_dsc)
            .current_dir(&dir)
            .env("SBUILD_CONFIG", &sbuild_conf_file);

        Ok(thread::spawn(move || {
            command
                .status()
                .and_then(status_err)?;

            fs::rename(&dir, &complete_dir)?;

            Ok(complete_dir)
        }))
    }

    pub fn build<P: AsRef<Path>>(&self, arch: &Arch, dist: &str, sbuild_archs: &[&str], dir: P) -> io::Result<Vec<thread::JoinHandle<io::Result<PathBuf>>>> {
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
            rebuild: false,
            retry: false,
        };

        let source_dsc = self.source(&config)?;

        let mut threads = Vec::new();
        for sbuild_arch in sbuild_archs {
            println!("    - sbuild {}", sbuild_arch);
            threads.push(self.sbuild_thread(&source_dsc, sbuild_arch, &config)?);
        }

        Ok(threads)
    }
}
