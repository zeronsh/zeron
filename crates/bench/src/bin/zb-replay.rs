fn main() -> std::process::ExitCode {
    #[cfg(unix)]
    let result = zeron_bench::replay::cli::main();
    #[cfg(not(unix))]
    let result: Result<(), &str> = Err("zb-replay requires Unix");
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("zb-replay: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
