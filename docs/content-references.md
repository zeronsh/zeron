# Content references

The shared Markdown parser recognizes complete `:codex-file-citation{...}`
single-line directives outside code, HTML, and existing links. `path` must be an absolute
file path; `purpose`, when present, must be `source` or `output`. Unknown or
incomplete directives remain ordinary text.

References become ordinary file links with a filename label. Existing transcript
file navigation resolves them in the originating chat's device and checkout,
including read-only absolute files outside the checkout. This does not install
skills, alter harness output, or rewrite the stored transcript. Copying retains
the destination path, following the existing abbreviated file-link behavior.

Compatibility spelling belongs in `zeron-markdown::content_reference`, shared
by desktop and mobile. File reads and navigation remain host responsibilities.
Arbitrary HTML in Markdown is still literal text.

Validation: `cargo test --locked -p zeron-markdown`. Coverage includes punctuation
and Unicode in paths, code examples, malformed directives, source offsets after
references, and full/incremental parity at every character boundary.
