//! Deterministic synthetic Markdown content.
use rand_chacha::{
    ChaCha8Rng,
    rand_core::{RngCore, SeedableRng},
};

pub fn generate(seed: u64, bytes: usize) -> String {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let paragraphs = [
        "# Design notes\n\nThe system records each change and checks the result. See [the guide](https://example.invalid/guide) for details.\n\n",
        "- Read the input\n- Validate the state\n- Report the outcome\n\n",
        "```rust\nfn inspect(value: usize) -> usize { value.saturating_add(1) }\n```\n\n",
        "```ts\nconst ready = (items: string[]) => items.length > 0;\n```\n\n",
        "```sh\nprintf '%s\\n' ready\n```\n\n",
    ];
    let mut text = String::with_capacity(bytes);
    while text.len() < bytes {
        let item = paragraphs[(rng.next_u32() as usize) % paragraphs.len()];
        let take = (bytes - text.len()).min(item.len());
        text.push_str(&item[..take]);
    }
    text
}
