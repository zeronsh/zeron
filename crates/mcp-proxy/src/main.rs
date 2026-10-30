fn main() -> std::io::Result<()> {
    zeron_mcp_proxy::run_cli(std::env::args().skip(1), zeron_mcp_proxy::data_dir())
}
