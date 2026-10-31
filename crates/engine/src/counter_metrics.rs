//! On-demand allocator metrics; never sampled in the engine's hot paths.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn parse_malloc_info(xml: &str) -> Option<serde_json::Value> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    if !root.has_tag_name("malloc") {
        return None;
    }
    let mut arenas = 0u64;
    let (mut system, mut fast, mut rest, mut mapped) = (None, None, None, None);
    for node in root.children().filter(|n| n.is_element()) {
        if node.has_tag_name("heap") {
            arenas = arenas.checked_add(1)?;
        } else {
            let slot = match (node.tag_name().name(), node.attribute("type")) {
                ("system", Some("current")) => &mut system,
                ("total", Some("fast")) => &mut fast,
                ("total", Some("rest")) => &mut rest,
                ("total", Some("mmap")) => &mut mapped,
                _ => continue,
            };
            if slot.is_some() {
                return None;
            }
            *slot = Some(node.attribute("size")?.parse::<u64>().ok()?);
        }
    }
    if arenas == 0 {
        return None;
    }
    let system = system?.checked_add(mapped?)?;
    let free = fast?.checked_add(rest?)?;
    Some(
        serde_json::json!({"arenas": arenas, "system_bytes": system, "in_use_bytes": system.checked_sub(free)?, "free_bytes": free}),
    )
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(crate) fn malloc() -> Option<serde_json::Value> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SAMPLING: AtomicBool = AtomicBool::new(false);
    if SAMPLING
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return None;
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            SAMPLING.store(false, Ordering::Release);
        }
    }
    let _guard = Guard;
    // malloc_info holds allocator locks while writing. A bounded, preallocated
    // fmemopen stream avoids open_memstream reallocating under those locks.
    let mut buffer = vec![0u8; 8 * 1024 * 1024];
    let mut io_buffer = [0u8; 4096];
    unsafe {
        let stream = libc::fmemopen(buffer.as_mut_ptr().cast(), buffer.len(), c"w".as_ptr());
        if stream.is_null() {
            return None;
        }
        if libc::setvbuf(
            stream,
            io_buffer.as_mut_ptr().cast(),
            libc::_IOFBF,
            io_buffer.len(),
        ) != 0
        {
            libc::fclose(stream);
            return None;
        }
        let status = libc::malloc_info(0, stream);
        let flush_status = libc::fflush(stream);
        let size = libc::ftell(stream);
        let error = libc::ferror(stream);
        let close_status = libc::fclose(stream);
        if status != 0
            || flush_status != 0
            || error != 0
            || close_status != 0
            || size < 0
            || size as usize >= buffer.len()
        {
            return None;
        }
        std::str::from_utf8(&buffer[..size as usize])
            .ok()
            .and_then(parse_malloc_info)
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
        let xml = r#"<malloc version="1"><heap nr="0"><system type="current" size="400"/></heap><heap nr="1"/><total size="10" count="1" type="fast"/><total type="rest" size="30"/><total type="mmap" size="100"/><system size="400" type="current"/></malloc>"#;
        assert_eq!(
            parse_malloc_info(xml).unwrap(),
            serde_json::json!({"arenas":2,"system_bytes":500,"in_use_bytes":460,"free_bytes":40})
        );
        assert!(parse_malloc_info("<malloc/>").is_none());
        assert!(parse_malloc_info(&xml.replace("size=\"30\"", "size=\"no\"")).is_none());
        assert!(
            parse_malloc_info(
                &xml.replace("</malloc>", "<total type=\"rest\" size=\"1\"/></malloc>")
            )
            .is_none()
        );
        assert!(parse_malloc_info(&xml.replace("</malloc>", "")).is_none());
        assert!(malloc().is_some());
    }
}
