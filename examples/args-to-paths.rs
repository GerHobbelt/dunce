// spell-checker:ignore (jargon) canonicalize

fn main() -> std::process::ExitCode {
    let args: Vec<_> = wild::args_os().collect();
    println!("args {:?}", args);

    args.iter().for_each(|arg| {
        let std_canon_arg = std::fs::canonicalize(arg);
        let dunce_canon_arg = dunce::canonicalize(arg);
        // let canon_arg =
        //     fs::canonicalize(arg, fs::MissingHandling::Normal, fs::ResolveMode::Physical);
        // let normal_arg = fs::normalize_path(Path::new(arg));
        println!(
            "arg {:?} => std_canon {:?}, dunce_canon {:?}",
            arg, std_canon_arg, dunce_canon_arg /* , canon_arg, normal_arg */
        );
    });

    std::process::ExitCode::SUCCESS
}
