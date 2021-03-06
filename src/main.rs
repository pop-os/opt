use pop_opt::{
    Arch,
    Pkg,
    ensure_dir,
    ensure_dir_clean,
    status_err,
};
use std::{
    collections::BTreeMap,
    env,
    fmt::Write,
    fs,
    io,
    path::Path,
    process,
};

pub extern "C" fn interrupt(_signal: i32) {}

fn build(arch: &Arch, args: &[String]) -> io::Result<()> {
    //TODO: passed as argument and used in pkg.build
    let sbuild_dist = "focal";
    //TODO: get dynamically
    let sbuild_dist_version = "20.04";
    let sbuild_archs = ["amd64", "i386"];

    let build_parent_dir = ensure_dir("build")?;
    let sbuild_arch_dir = ensure_dir(build_parent_dir.join(&arch.name))?;
    let build_dir = ensure_dir(sbuild_arch_dir.join(sbuild_dist))?;

    let repo_parent_dir = ensure_dir("repo")?;
    let repo_dir = ensure_dir_clean(repo_parent_dir.join(&arch.name))?;

    let dists_parent_dir = ensure_dir(repo_dir.join("dists"))?;
    let dists_dir = ensure_dir(dists_parent_dir.join(sbuild_dist))?;
    let comp_dir = ensure_dir(dists_dir.join("main"))?;

    let pool_parent_dir = ensure_dir(repo_dir.join("pool"))?;
    let pool_dir = ensure_dir(pool_parent_dir.join(sbuild_dist))?;

    let mut pkg_threads = BTreeMap::new();

    let pkgs = Pkg::load_all("pkg")?;
    for pkg in pkgs.iter() {
        if ! args.is_empty() && ! args.contains(&pkg.name) {
            println!("- skipping {}", pkg.name);
            continue;
        }

        let pkg_build_dir = ensure_dir(build_dir.join(&pkg.name))?;
        let threads = pkg.build(arch, sbuild_dist, &sbuild_archs, &pkg_build_dir)?;
        pkg_threads.insert(pkg.name.clone(), threads);
    }

    for pkg in pkgs.iter() {
        if let Some(threads) = pkg_threads.remove(&pkg.name) {
            let mut debs = Vec::new();
            for thread in threads {
                match thread.join().unwrap() {
                    Ok(sbuild_dir) => for entry_res in fs::read_dir(&sbuild_dir)? {
                        let entry = entry_res?;
                        if entry.file_name().to_str().unwrap_or("").ends_with(".deb") {
                            debs.push(entry.path());
                        }
                    },
                    Err(err) => {
                        println!("- {}: {}", pkg.name, err);
                    }
                }
            }

            let pkg_pool_dir = ensure_dir(pool_dir.join(&pkg.name))?;
            for deb in debs {
                let pool_deb = pkg_pool_dir.join(&deb.file_name().unwrap());
                if ! pool_deb.is_file() {
                    fs::hard_link(&deb, &pool_deb)?;
                }
            }
        }
    }


    for sbuild_arch in sbuild_archs.iter() {
        let binary_dir = ensure_dir(comp_dir.join(format!("binary-{}", sbuild_arch)))?;

        let output = process::Command::new("apt-ftparchive")
            .arg("--arch").arg(sbuild_arch)
            .arg("packages")
            .arg(&pool_dir.strip_prefix(&repo_dir).unwrap())
            .current_dir(&repo_dir)
            .stdout(process::Stdio::piped())
            .spawn()?
            .wait_with_output()?;
        status_err(output.status)?;

        let packages_file = binary_dir.join("Packages");
        fs::write(&packages_file, &output.stdout)?;

        process::Command::new("gzip")
            .arg("--keep")
            .arg(packages_file)
            .status()
            .and_then(status_err)?;

        let mut release = String::new();
        writeln!(release, "Archive: {}", sbuild_dist).unwrap();
        writeln!(release, "Version: {}", sbuild_dist_version).unwrap();
        writeln!(release, "Component: main").unwrap();
        writeln!(release, "Origin: pop-os-opt-{}", arch.name).unwrap();
        writeln!(release, "Label: Pop!_OS Opt {}", arch.name).unwrap();
        writeln!(release, "Architecture: {}", sbuild_arch).unwrap();
        fs::write(binary_dir.join("Release"), &release)?;
    }

    let output = process::Command::new("apt-ftparchive")
        .arg("-o").arg(format!("APT::FTPArchive::Release::Origin=pop-os-opt-{}", arch.name))
        .arg("-o").arg(format!("APT::FTPArchive::Release::Label=Pop!_OS Opt {}", arch.name))
        .arg("-o").arg(format!("APT::FTPArchive::Release::Suite={}", sbuild_dist))
        .arg("-o").arg(format!("APT::FTPArchive::Release::Version={}", sbuild_dist_version))
        .arg("-o").arg(format!("APT::FTPArchive::Release::Codename={}", sbuild_dist))
        .arg("-o").arg(format!("APT::FTPArchive::Release::Architectures={}", sbuild_archs.join(" ")))
        .arg("-o").arg("APT::FTPArchive::Release::Components=main")
        .arg("-o").arg(format!(
            "APT::FTPArchive::Release::Description=Pop!_OS Opt {} {} {}",
            sbuild_dist,
            sbuild_dist_version,
            arch.name
        ))
        .arg("release")
        .arg(".")
        .current_dir(&dists_dir)
        .stdout(process::Stdio::piped())
        .spawn()?
        .wait_with_output()?;
    status_err(output.status)?;

    let release_file = dists_dir.join("Release");
    fs::write(&release_file, &output.stdout)?;

    //TODO: --local-user
    process::Command::new("gpg")
        .arg("--clearsign")
        .arg("--batch").arg("--yes")
        .arg("--digest-algo").arg("sha512")
        .arg("-o").arg(dists_dir.join("InRelease"))
        .arg(&release_file)
        .status()
        .and_then(status_err)?;

    //TODO: --local-user
    process::Command::new("gpg")
        .arg("-abs")
        .arg("--batch").arg("--yes")
        .arg("--digest-algo").arg("sha512")
        .arg("-o").arg(dists_dir.join("Release.gpg"))
        .arg(&release_file)
        .status()
        .and_then(status_err)?;

    Ok(())
}

fn chroot(_arch: &Arch) -> io::Result<()> {
    //TODO: passed as argument
    let sbuild_dist = "focal";
    let sbuild_archs = ["amd64", "i386"];
    let mirror = "http://archive.ubuntu.com/ubuntu";

    let parent_dir = Path::new("/srv/chroot");
    for sbuild_arch in sbuild_archs.iter() {
        let name = format!("{}-{}-popopt", sbuild_dist, sbuild_arch);
        println!("- chroot {}", name);
        let dir = parent_dir.join(&name);
        if ! dir.is_dir() {
            process::Command::new("sudo")
                .arg("sbuild-createchroot")
                .arg(format!("--arch={}", sbuild_arch))
                .arg("--chroot-suffix=-popopt")
                .arg("--components=main,restricted,universe,multiverse")
                .arg(format!("--extra-repository=deb {} {}-updates main restricted universe multiverse", mirror, sbuild_dist))
                .arg(format!("--extra-repository=deb-src {} {}-updates main restricted universe multiverse", mirror, sbuild_dist))
                .arg(format!("--extra-repository=deb {} {}-security main restricted universe multiverse", mirror, sbuild_dist))
                .arg(format!("--extra-repository=deb-src {} {}-security main restricted universe multiverse", mirror, sbuild_dist))
                .arg(sbuild_dist)
                .arg(&dir)
                .arg(mirror)
                .status()
                .and_then(status_err)?;
        }

        process::Command::new("sudo")
            .arg("sbuild-update")
            .arg("--update")
            .arg("--dist-upgrade")
            .arg("--clean")
            .arg("--autoclean")
            .arg("--autoremove")
            .arg(format!("--arch={}", sbuild_arch))
            .arg(&name)
            .status()
            .and_then(status_err)?;
    }

    Ok(())
}

fn repo(arch: &Arch, args: &[String]) -> io::Result<()> {
    let remove = args.contains(&"-r".to_string());

    let url = format!("https://apt.pop-os.org/opt/{}/", arch.name);
    println!("- {} {}", if remove { "Removing" } else { "Adding" }, url);

    //TODO: something better than this preferences hack to remove opt packages
    let pref_file = Path::new("/etc/apt/preferences.d/popopt");
    if remove {
        process::Command::new("sudo")
            .arg("bash")
            .arg("-c")
            .arg(format!(
                "echo 'Package: *\nPin: release o=Ubuntu\nPin-Priority: 1000' > '{}'",
                pref_file.display()
            ))
            .status()
            .and_then(status_err)?;

        process::Command::new("sudo")
            .arg("apt-get")
            .arg("upgrade")
            .arg("--yes")
            .arg("--allow-downgrades")
            .status()
            .and_then(status_err)?;
    }

    process::Command::new("sudo")
        .arg("rm")
        .arg("--force")
        .arg("--verbose")
        .arg(&pref_file)
        .status()
        .and_then(status_err)?;

    let source_file = Path::new("/etc/apt/sources.list.d/popopt.list");
    if remove {
        process::Command::new("sudo")
            .arg("rm")
            .arg("--force")
            .arg("--verbose")
            .arg(&source_file)
            .status()
            .and_then(status_err)?;
    } else {
        let os_release = os_release::OsRelease::new()?;
        let source = format!("deb {} {} main", url, os_release.version_codename);

        process::Command::new("sudo")
            .arg("bash")
            .arg("-c")
            .arg(format!(
                "echo '{}' > '{}'",
                source,
                source_file.display()
            ))
            .status()
            .and_then(status_err)?;
    }

    process::Command::new("sudo")
        .arg("apt-get")
        .arg("update")
        .status()
        .and_then(status_err)?;

    process::Command::new("sudo")
        .arg("apt-get")
        .arg("upgrade")
        .arg("--yes")
        .status()
        .and_then(status_err)?;

    Ok(())
}

fn pop_opt(args: &[String]) -> io::Result<()> {
    let cpu_features = Arch::cpu_features()?;
    println!("CPU features: {:?}", cpu_features);
    println!();

    let archs = Arch::load_all("arch/x86_64")?;
    let mut highest = None;
    for arch in archs {
        match arch.check_features(&cpu_features) {
            Ok(()) => {
                println!("{}: Supported", arch.name);
                highest = Some(arch);
            },
            Err(missing) => {
                println!("{}: Missing {:?}", arch.name, missing);
            }
        }
    }

    let arch = match highest {
        Some(some) => some,
        None => return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no optimization level found"
        ))
    };

    println!();
    println!("{}: Highest arch found", arch.name);
    println!("cflags: {:?}", arch.cflags());
    println!("rustflags: {:?}", arch.rustflags());
    println!();

    match args.get(0).map(|x| x.as_str()) {
        None => Ok(()),
        Some("build") => build(&arch, &args[1..]),
        Some("chroot") => chroot(&arch),
        Some("repo") => repo(&arch, &args[1..]),
        Some(arg) => Err(io::Error::new(
            io::ErrorKind::Other,
            format!("unknown subcommand '{}'", arg)
        ))
    }
}

fn main() {
    if unsafe { libc::signal(libc::SIGINT, interrupt as libc::sighandler_t) == libc::SIG_ERR } {
        panic!("failed to handle SIGINT");
    }

    let args: Vec<String> = env::args().skip(1).collect();
    match pop_opt(&args) {
        Ok(()) => (),
        Err(err) => {
            eprintln!("pop-opt {:?}: {}", args, err);
            process::exit(1);
        }
    }
}
