// Writes desktop.outline: what Zeron's desktop parser (crates/markdown)
// makes of each fixture, as the outline desktop.test.ts compares with the
// site's. Not built with the site. To remake it, from a scratch folder:
//
//   cargo new outline && cd outline
//   # Cargo.toml: [workspace] (empty), and under [dependencies]:
//   #   zeron-markdown = { path = "<repo>/crates/markdown" }
//   cp <this file> src/main.rs
//   cargo run -q -- <fixtures>/{focus-timer,screenshot-tray,short,notes,extremes,subset}.md > <fixtures>/desktop.outline
//
use zeron_markdown::{parse_full, Block, InlineRun};

fn squash(s: &str) -> String { s.split_whitespace().collect::<Vec<_>>().join(" ") }

fn inline(runs: &[InlineRun], out: &mut Vec<String>, pad: &str) -> String {
    let mut text = String::new();
    let mut i = 0;
    while i < runs.len() {
        let r = &runs[i];
        if let Some(img) = &r.style.image {
            out.push(format!("{pad}  picture {:?} <- {}{}", squash(&img.alt), img.source, img.link.as_ref().map(|l| format!(" in {l}")).unwrap_or_default()));
            i += 1;
            continue;
        }
        if r.style.task.is_some() { i += 1; continue; }
        if let Some(link) = &r.style.link {
            let mut words = String::new();
            while i < runs.len() && runs[i].style.link.as_ref() == Some(link) {
                if runs[i].style.image.is_some() {
                    let img = runs[i].style.image.as_ref().unwrap();
                    out.push(format!("{pad}  picture {:?} <- {} in {}", squash(&img.alt), img.source, link));
                } else { words.push_str(&runs[i].text); text.push_str(&runs[i].text); }
                i += 1;
            }
            out.push(format!("{pad}  link {:?} -> {}", squash(&words), link));
            continue;
        }
        text.push_str(&r.text);
        i += 1;
    }
    squash(&text)
}

fn task(runs: &[InlineRun]) -> &'static str {
    match runs.iter().find_map(|r| r.style.task.as_ref()) { Some(t) if t.checked => "done", Some(_) => "open", None => "none" }
}

fn blocks(list: &[Block], out: &mut Vec<String>, depth: usize) {
    let pad = "  ".repeat(depth);
    for b in list {
        match b {
            Block::Paragraph { runs } => { let mut sub = Vec::new(); let t = inline(runs, &mut sub, &pad); out.push(format!("{pad}p {t:?}")); out.extend(sub); }
            Block::Heading { level, runs } => { let mut sub = Vec::new(); let t = inline(runs, &mut sub, &pad); out.push(format!("{pad}h{level} {t:?}")); out.extend(sub); }
            Block::CodeBlock { language, code } => out.push(format!("{pad}code[{}] {:?}", language.clone().unwrap_or_default(), code.trim_end_matches('\n'))),
            Block::BlockQuote { children } => { out.push(format!("{pad}quote")); blocks(children, out, depth + 1); }
            Block::List { ordered_start, items } => {
                out.push(format!("{pad}list {}", ordered_start.map(|n| format!("ordered from {n}")).unwrap_or("bulleted".into())));
                for item in items {
                    let t = match item.first() { Some(Block::Paragraph { runs }) => task(runs), _ => "none" };
                    out.push(format!("{pad}  item task={t}"));
                    blocks(item, out, depth + 2);
                }
            }
            Block::Table { header, rows, align } => {
                let mut sub = Vec::new();
                let head: Vec<String> = header.iter().map(|c| inline(c, &mut sub, &pad)).collect();
                out.push(format!("{pad}table {:?} align {:?}", head, align.iter().map(|a| format!("{a:?}").to_lowercase()).collect::<Vec<_>>()));
                for row in rows { let r: Vec<String> = row.iter().map(|c| inline(c, &mut sub, &pad)).collect(); out.push(format!("{pad}  row {r:?}")); }
                out.extend(sub);
            }
            Block::Math { tex } => out.push(format!("{pad}math {tex:?}")),
            Block::Rule => out.push(format!("{pad}rule")),
        }
    }
}

fn main() {
    for path in std::env::args().skip(1) {
        let text = std::fs::read_to_string(&path).unwrap();
        let tree = parse_full(&text);
        let list: Vec<Block> = tree.blocks.iter().map(|t| t.block.clone()).collect();
        let mut out = Vec::new();
        blocks(&list, &mut out, 0);
        println!("=== {}", std::path::Path::new(&path).file_name().unwrap().to_string_lossy());
        for line in out { println!("{line}"); }
    }
}
