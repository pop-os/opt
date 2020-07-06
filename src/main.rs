use pop_opt::{
    Arch,
    Pkg,
    ensure_dir,
};
use std::{
    io,
    process,
};

fn pkg(arch: &Arch) -> io::Result<()> {
    let build_dir = ensure_dir("build")?;
    let pkgs = Pkg::load_all("pkg")?;
    for pkg in pkgs {
        let pkg_build_dir = ensure_dir(&build_dir.join(&pkg.name))?;
        pkg.build(arch, &pkg_build_dir)?;
    }

    Ok(())
}

fn arch() -> io::Result<()> {
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

    if let Some(arch) = highest {
        println!();
        println!("{}: Highest arch found", arch.name);
        println!("cflags: {:?}", arch.cflags());
        println!("rustflags: {:?}", arch.rustflags());

        println!();
        pkg(&arch)?;
    }

    Ok(())
}

fn main() {
    match arch() {
        Ok(()) => (),
        Err(err) => {
            eprintln!("pop-opt: {}", err);
            process::exit(1);
        }
    }
}
