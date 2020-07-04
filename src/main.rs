use pop_opt::Arch;

fn main() {
    let cpu_features = Arch::cpu_features().unwrap();
    println!("CPU features: {:?}", cpu_features);
    println!();

    let archs = Arch::load_all("arch/x86_64").unwrap();
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
    }
}
