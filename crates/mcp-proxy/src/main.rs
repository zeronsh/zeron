fn main() -> std::io::Result<()> {
    let argv: Vec<_> = std::env::args_os().collect();
    if zeron_mcp_proxy::shim::invoked_path(&argv[0]).file_stem()
        != Some(std::ffi::OsStr::new("zeron-mcp-proxy"))
    {
        return zeron_mcp_proxy::shim::run(argv);
    }
    if argv.get(1).is_some_and(|a| a == "--shim-version") && argv.len() == 2 {
        println!("1");
        return Ok(());
    }
    zeron_mcp_proxy::run_cli(std::env::args().skip(1), zeron_mcp_proxy::data_dir())
}
