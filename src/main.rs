use pop_opt::Arch;

fn main() {
    let cpu_features = Arch::cpu_features().unwrap();
    println!("CPU features: {:?}", cpu_features);

    let archs = Arch::load_all("arch/x86_64").unwrap();
    for arch in archs {
        match arch.check_features(&cpu_features) {
            Ok(()) => {
                println!("{}: Supported", arch.name);
            },
            Err(missing) => {
                println!("{}: Missing {:?}", arch.name, missing);
            }
        }
    }
}
