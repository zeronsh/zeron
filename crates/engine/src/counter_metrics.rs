//! On-demand allocator metrics; never sampled in the engine's hot paths.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn parse_malloc_info(xml: &str) -> Option<serde_json::Value> {
    fn attr(line: &str, key: &str) -> Option<u64> {
        line.split_whitespace().find_map(|part| {
            part.strip_prefix(key)?
                .strip_prefix("=\"")?
                .trim_end_matches(['"', '/', '>'])
                .parse()
                .ok()
        })
    }
    let mut arenas = 0u64;
    let mut depth = 0u32;
    let (mut system, mut free, mut mapped) = (None, 0u64, 0u64);
    for line in xml.lines().map(str::trim) {
        if line.starts_with("<heap ") {
            arenas += 1;
            depth += 1;
        }
        if line.starts_with("</heap>") {
            depth = depth.checked_sub(1)?;
        }
        if depth == 0 && line.starts_with("<system ") && line.contains("type=\"current\"") {
            system = Some(attr(line, "size")?);
        }
        if depth == 0 && line.starts_with("<total ") {
            if line.contains("type=\"fast\"") || line.contains("type=\"rest\"") {
                free = free.checked_add(attr(line, "size")?)?;
            }
            if line.contains("type=\"mmap\"") {
                mapped = attr(line, "size")?;
            }
        }
    }
    let system = system?.checked_add(mapped)?;
    Some(
        serde_json::json!({"arenas": arenas, "system_bytes": system, "in_use_bytes": system.checked_sub(free)?, "free_bytes": free}),
    )
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(crate) fn malloc() -> Option<serde_json::Value> {
    unsafe {
        let mut buffer = std::ptr::null_mut();
        let mut len = 0;
        let stream = libc::open_memstream(&mut buffer, &mut len);
        if stream.is_null() {
            return None;
        }
        let status = libc::malloc_info(0, stream);
        let close_status = libc::fclose(stream);
        let result = if status == 0 && close_status == 0 && !buffer.is_null() {
            std::str::from_utf8(std::slice::from_raw_parts(buffer.cast::<u8>(), len))
                .ok()
                .and_then(parse_malloc_info)
        } else {
            None
        };
        libc::free(buffer.cast());
        result
    }
}
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub(crate) fn malloc() -> Option<serde_json::Value> {
    None
}

#[cfg(all(test, target_os = "linux", target_env = "gnu"))]
mod tests {
    use super::*;
    #[test]
    fn engine_counters_malloc_info_parsing() {
        let xml = "<malloc version=\"1\">\n<heap nr=\"0\">\n<system type=\"current\" size=\"400\"/>\n</heap>\n<heap nr=\"1\">\n</heap>\n<total type=\"fast\" count=\"1\" size=\"10\"/>\n<total type=\"rest\" count=\"2\" size=\"30\"/>\n<total type=\"mmap\" count=\"1\" size=\"100\"/>\n<system type=\"current\" size=\"400\"/>\n</malloc>";
        assert_eq!(
            parse_malloc_info(xml).unwrap(),
            serde_json::json!({"arenas":2,"system_bytes":500,"in_use_bytes":460,"free_bytes":40})
        );
        assert!(parse_malloc_info("<malloc/>").is_none());
        assert!(malloc().is_some());
    }
}
