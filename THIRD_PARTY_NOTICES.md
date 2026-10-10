# Third-party notices

Zeron bundles the following syntax-highlighting components. Unless noted otherwise, their parsers and queries are consumed from the pinned Rust crates listed in `Cargo.lock`. The Kotlin highlight query is maintained as Zeron source code and is not attributed to the grammar crate.

| Component | Version | License | Source |
| --- | --- | --- | --- |
| Tree-sitter | 0.26.11 | MIT | https://github.com/tree-sitter/tree-sitter |
| Tree-sitter highlight | 0.26.11 | MIT | https://github.com/tree-sitter/tree-sitter |
| Tree-sitter Rust grammar and queries | 0.24.2 | MIT | https://github.com/tree-sitter/tree-sitter-rust |
| Tree-sitter JavaScript grammar and queries | 0.25.0 | MIT | https://github.com/tree-sitter/tree-sitter-javascript |
| Tree-sitter TypeScript grammar and queries | 0.23.2 | MIT | https://github.com/tree-sitter/tree-sitter-typescript |
| Tree-sitter Python, Go, JSON, Bash, HTML, CSS, C, C++, C#, Java, Ruby and PHP grammars and queries | pinned in `Cargo.lock` | MIT | https://github.com/tree-sitter |
| Tree-sitter TOML, Markdown, YAML, Swift, SQL, Lua, Nix, Make and Containerfile grammars and queries | pinned in `Cargo.lock` | MIT-compatible; see each crate | Crate repositories recorded in `Cargo.lock` |
| Tree-sitter Kotlin grammar | 1.1.0 | MIT | https://github.com/tree-sitter-grammars/tree-sitter-kotlin |

Zeron also uses the following editor foundations from the pinned `zeronsh/gpui-component` fork. The fork aligns these crates with the same GPUI revision used by Comet.

| Component | Version | License | Source |
| --- | --- | --- | --- |
| gpui-base | 0.5.2 (`ed27327`) | Apache-2.0 | https://github.com/zeronsh/gpui-component |
| mermaid-rs-renderer | 0.3.1 | MIT | https://github.com/1jehuang/mermaid-rs-renderer |
| Ropey | 2.0.0-beta.1 | MIT | https://github.com/cessen/ropey |

Zeron's own source code is licensed under the terms in `LICENSE`. Bundled third-party components retain their respective licenses and notices.

## Symbols

Zeron bundles the SVG icon set and filename/folder associations from
[Symbols](https://github.com/miguelsolorio/vscode-symbols/tree/296ef1b62287fb2315cb5651e552e09e8c8e1de8).
Symbols is MIT licensed. The complete upstream license and copyright notice is
retained at `crates/ui/assets/file-icons/LICENSE.symbols`.

## Bundled theme palette adaptations

Zeron includes manually curated palette adaptations derived from the projects
below. The source repository and exact audited revision are also embedded in
each resolved theme variant. These projects are not affiliated with or endorsed
by Zeron. Their names identify the corresponding palette adaptations.

| Theme project | Audited revision | License and upstream notice |
| --- | --- | --- |
| Visual Studio Code Dark+/Light+ | `e33d147d4c0fa65ce17cb73ec9d798f064b4bf1f` | [MIT](https://github.com/microsoft/vscode/blob/e33d147d4c0fa65ce17cb73ec9d798f064b4bf1f/LICENSE.txt) |
| Catppuccin for VS Code | `befc9e6fc41980f4241408f7049755d47c06ff45` | [MIT](https://github.com/catppuccin/vscode/blob/befc9e6fc41980f4241408f7049755d47c06ff45/LICENSE) |
| Tokyo Night VS Code Theme | `7c0f11eaef322f293621ca7befe462214b7ea468` | [MIT](https://github.com/tokyo-night/tokyo-night-vscode-theme/blob/7c0f11eaef322f293621ca7befe462214b7ea468/LICENSE.txt) |
| Dracula for Visual Studio Code | `1b9ecf4d7e0c8cc2e2e890a7a41ad1db5fff1e6c` | [MIT](https://github.com/dracula/visual-studio-code/blob/1b9ecf4d7e0c8cc2e2e890a7a41ad1db5fff1e6c/LICENSE) |
| GitHub VS Code Theme | `cd78e5e4e7bcf132a6f428ae0f32264bb1b729cf` | [MIT](https://github.com/primer/github-vscode-theme/blob/cd78e5e4e7bcf132a6f428ae0f32264bb1b729cf/LICENSE) |
| Ayu for VS Code | `444ef92911cb75c3933c8003e3a7c79b6b6c914f` | [MIT](https://github.com/ayu-theme/vscode-ayu/blob/444ef92911cb75c3933c8003e3a7c79b6b6c914f/LICENSE) |
| Gruvbox Theme | `ca3b8ad203e84a884ca33fb84b5795cf43032709` | [MIT](https://github.com/jdinhify/vscode-theme-gruvbox/blob/ca3b8ad203e84a884ca33fb84b5795cf43032709/LICENSE) |
| Rosé Pine for VS Code | `d8f5ebe8e096fa833e997c07eb7685ee1677a4ba` | [MIT](https://github.com/rose-pine/vscode/blob/d8f5ebe8e096fa833e997c07eb7685ee1677a4ba/LICENSE) |
| Nord Visual Studio Code | `8ead09822c02d0d49d0f764104505e5a34d3689f` | [MIT](https://github.com/nordtheme/visual-studio-code/blob/8ead09822c02d0d49d0f764104505e5a34d3689f/license) |
| One Dark Pro | `e6ccf638d5b69aa38cd1005edb0ee7ba7ef6fedc` | [MIT](https://github.com/Binaryify/OneDark-Pro/blob/e6ccf638d5b69aa38cd1005edb0ee7ba7ef6fedc/LICENSE.txt) |
| Atom One Dark Theme | `a8be970644982221f9b61fb1c4b3da74b4beab79` | [MIT](https://github.com/akamud/vscode-theme-onedark/blob/a8be970644982221f9b61fb1c4b3da74b4beab79/LICENSE) |
| Night Owl | `cc291eba7976b20d7c66bde6883c27b902196b07` | [MIT](https://github.com/sdras/night-owl-vscode-theme/blob/cc291eba7976b20d7c66bde6883c27b902196b07/LICENSE.md) |
| Winter is Coming | `260547834cb6ac37dd5b8bb5842cc1c8d3164946` | [MIT](https://github.com/johnpapa/vscode-winteriscoming/blob/260547834cb6ac37dd5b8bb5842cc1c8d3164946/LICENSE.md) |
| Palenight Theme | `6291efaace90855abe3d79025327ca41b9a3138c` | [MIT](https://github.com/whizkydee/vscode-palenight-theme/blob/6291efaace90855abe3d79025327ca41b9a3138c/license.md) |
| SynthWave '84 | `ecfa2fe1279f7233663fa3f98a96e6756000567b` | [MIT](https://github.com/robb0wen/synthwave-vscode/blob/ecfa2fe1279f7233663fa3f98a96e6756000567b/LICENSE) |
| Shades of Purple | `e8eb49f33e5db05ceba6677367b33ddb27ad821c` | [MIT text with an additional “With condition” section](https://github.com/ahmadawais/shades-of-purple-vscode/blob/e8eb49f33e5db05ceba6677367b33ddb27ad821c/LICENSE.md); Zeron is MIT-licensed, satisfying the stated condition |
| Cobalt2 | `c4e9574372b85afad1682ed0fdd1ac0411c62512` | [MIT](https://github.com/wesbos/cobalt2-vscode/blob/c4e9574372b85afad1682ed0fdd1ac0411c62512/LICENSE) |
| Andromeda | `d1abb48c69493000aa0133a32d594eb25e523d4f` | [MIT](https://github.com/EliverLara/Andromeda/blob/d1abb48c69493000aa0133a32d594eb25e523d4f/LICENSE.md) |

The palette values are adapted under the corresponding upstream license. The
linked license pages contain each project's copyright and permission notice and
are pinned to the same revision as the adapted source.

Copyright notices retained from those pinned upstream licenses:

- Copyright (c) 2015 - present Microsoft Corporation
- Copyright (c) 2021 Catppuccin
- Copyright (c) 2018-present Enkia
- Copyright (c) 2016 Dracula Theme
- Copyright (c) 2020 Primer
- Copyright (c) 2016 Ike Kurghinyan
- Copyright © 2017 JD
- Copyright (c) 2021 Rosé Pine
- Copyright (c) 2016-present Sven Greb <development@svengreb.de> (https://www.svengreb.de)
- Copyright (c) 2013-2022 Binaryify
- Copyright (c) 2015 Mahmoud Ali
- Copyright (c) 2018 Sarah Drasner
- Copyright (c) 2015-2017 JohnPapa.net, LLC
- Copyright (c) 2017-present Olaolu Olawuyi
- Copyright (c) 2019 Robb Owen
- Copyright (c) 2015-∞ Ahmad Awais
- Copyright (c) 2018 Wes Bos, Roberto Achar
- Copyright (c) 2017 <eliverlara@gmail.com>

The common MIT permission notice for the adaptations above follows:

> Permission is hereby granted, free of charge, to any person obtaining a copy
> of this software and associated documentation files (the “Software”), to deal
> in the Software without restriction, including without limitation the rights
> to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
> copies of the Software, and to permit persons to whom the Software is
> furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in
> all copies or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED “AS IS”, WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
> IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
> FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
> AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
> LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
> OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
> SOFTWARE.

The pinned Shades of Purple license additionally says that anything built with
it should also be MIT licensed. Zeron is distributed under MIT terms.

## mermaid-rs-renderer

MIT License

Copyright (c) 2026 mermaid-rs-renderer contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Native browser host

The macOS browser uses [Wry 0.56.0](https://github.com/tauri-apps/wry/tree/wry-v0.56.0)
(MIT OR Apache-2.0) to host the system WebKit engine, with the `objc2` family
of bindings (MIT) and `block2` (MIT). Exact versions and transitive dependencies
are pinned in `Cargo.lock`. The browser integration is independently written
Zeron code.

The Zui native overlay renderer adapts Apache-2.0 GPUI code from
[`egoist/zed` at `57bd4fe`](https://github.com/egoist/zed/tree/57bd4fe181639797d395978d5de17bc9e10a6219/crates/gpui_macos).
Attribution is retained in the pinned Zui dependency’s `NOTICE`.

## Optional desktop dictation

Parakeet TDT 0.6B v3 model weights are by NVIDIA under CC BY 4.0. The optional download uses Ivan Stupakov's INT8 ONNX conversion, pinned to revision `8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce`, with per-file SHA-256 verification. Original: https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3. Conversion: https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx. License: https://creativecommons.org/licenses/by/4.0/.

Native runtime dependencies: parakeet-rs 0.3.8 (MIT OR Apache-2.0), ort/ort-sys 2.0.0-rc.13 (MIT OR Apache-2.0), ONNX Runtime 1.28.0 (MIT), cpal 0.17.3 (Apache-2.0), and rubato 0.16.2 (MIT). Windows packages also bundle Microsoft DirectML (DirectML.dll) for optional GPU acceleration under Microsoft's DirectML redistributable license: https://www.nuget.org/packages/Microsoft.AI.DirectML. Apple silicon macOS and x86_64 Linux packages bundle Dawn (libwebgpu_dawn), Google's WebGPU implementation used by the ONNX Runtime WebGPU execution provider, under the BSD-3-Clause license: https://dawn.googlesource.com/dawn/+/refs/heads/main/LICENSE. See `crates/voice/NOTICE.md` for exact conversion provenance, changes, runtime/platform limitations and supported languages. Models are not included in the application bundle.

## Bezel thinking orbs

`crates/orb/` and `crates/ui/src/orb/` were extracted from Bezel
(https://github.com/clearloop/bezel), revision
`6141af9c16f7353cdf36003f7404e0a94566a163`, `crates/agent/src/orbs/`.
This code is compiled into the desktop app and, through the mobile core,
the iOS app; Bezel is not a dependency. The upstream module is a port of
gpui-thinking-orbs (https://github.com/FrancoEscob/gpui-thinking-orbs), itself
a port of Jakub Antalik's thinking-orbs
(https://github.com/Jakubantalik/thinking-orbs); the orb algorithms and
per-state tunings originate there. All three are MIT-licensed. Original
attribution comments are retained. The iOS app carries this notice as
`Voice/ThinkingOrbs-LICENSE.txt`.

MIT License

Copyright (c) 2026 clearloop
Copyright (c) 2026 Wing (portions extracted from zeronsh/comet)
Copyright (c) 2026 gpui-thinking-orbs contributors
Copyright (c) 2026 Jakub Antalik

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Codex native voice protocol

The bounded helper control protocol and native runtime environment in
`crates/voice-media/src/lib.rs` are adapted from OpenAI Codex
`rust-v0.159.0` (`377f7f557a6bdea0f3a2d26d4d899c66db4789d0`),
`codex-rs/realtime-webrtc/src/{protocol,client,linux_alsa}.rs`.
Copyright 2025 OpenAI. Licensed under the Apache License, Version 2.0.
Upstream NOTICE: "OpenAI Codex. Copyright 2025 OpenAI."
Zeron runs the helper of the user's separately installed standalone Codex, on
every platform; Zeron packages do not contain or redistribute Codex binaries or
its native audio runtime. This adaptation uses asynchronous bounded pipes and
the Zeron engine's owner leases. The iOS voice endpoint
(`apps/ios/Zeron/Voice/CodexVoicePeer.swift`) implements the transport behavior
described by `codex-rs/voice-host/src/transport.rs` at
`a956835d020762cb2b570053af06f643a11c0ecc` without copying its code.

Voice sessions use OpenAI's service under the user's own ChatGPT-authenticated
Codex account and OpenAI's terms; this license covers the adapted code only.
OpenAI, ChatGPT and Codex are trademarks of OpenAI. Zeron is not affiliated
with or endorsed by OpenAI. The license text follows.

                                 Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

1.  Definitions.

    "License" shall mean the terms and conditions for use, reproduction,
    and distribution as defined by Sections 1 through 9 of this document.

    "Licensor" shall mean the copyright owner or entity authorized by
    the copyright owner that is granting the License.

    "Legal Entity" shall mean the union of the acting entity and all
    other entities that control, are controlled by, or are under common
    control with that entity. For the purposes of this definition,
    "control" means (i) the power, direct or indirect, to cause the
    direction or management of such entity, whether by contract or
    otherwise, or (ii) ownership of fifty percent (50%) or more of the
    outstanding shares, or (iii) beneficial ownership of such entity.

    "You" (or "Your") shall mean an individual or Legal Entity
    exercising permissions granted by this License.

    "Source" form shall mean the preferred form for making modifications,
    including but not limited to software source code, documentation
    source, and configuration files.

    "Object" form shall mean any form resulting from mechanical
    transformation or translation of a Source form, including but
    not limited to compiled object code, generated documentation,
    and conversions to other media types.

    "Work" shall mean the work of authorship, whether in Source or
    Object form, made available under the License, as indicated by a
    copyright notice that is included in or attached to the work
    (an example is provided in the Appendix below).

    "Derivative Works" shall mean any work, whether in Source or Object
    form, that is based on (or derived from) the Work and for which the
    editorial revisions, annotations, elaborations, or other modifications
    represent, as a whole, an original work of authorship. For the purposes
    of this License, Derivative Works shall not include works that remain
    separable from, or merely link (or bind by name) to the interfaces of,
    the Work and Derivative Works thereof.

    "Contribution" shall mean any work of authorship, including
    the original version of the Work and any modifications or additions
    to that Work or Derivative Works thereof, that is intentionally
    submitted to Licensor for inclusion in the Work by the copyright owner
    or by an individual or Legal Entity authorized to submit on behalf of
    the copyright owner. For the purposes of this definition, "submitted"
    means any form of electronic, verbal, or written communication sent
    to the Licensor or its representatives, including but not limited to
    communication on electronic mailing lists, source code control systems,
    and issue tracking systems that are managed by, or on behalf of, the
    Licensor for the purpose of discussing and improving the Work, but
    excluding communication that is conspicuously marked or otherwise
    designated in writing by the copyright owner as "Not a Contribution."

    "Contributor" shall mean Licensor and any individual or Legal Entity
    on behalf of whom a Contribution has been received by Licensor and
    subsequently incorporated within the Work.

2.  Grant of Copyright License. Subject to the terms and conditions of
    this License, each Contributor hereby grants to You a perpetual,
    worldwide, non-exclusive, no-charge, royalty-free, irrevocable
    copyright license to reproduce, prepare Derivative Works of,
    publicly display, publicly perform, sublicense, and distribute the
    Work and such Derivative Works in Source or Object form.

3.  Grant of Patent License. Subject to the terms and conditions of
    this License, each Contributor hereby grants to You a perpetual,
    worldwide, non-exclusive, no-charge, royalty-free, irrevocable
    (except as stated in this section) patent license to make, have made,
    use, offer to sell, sell, import, and otherwise transfer the Work,
    where such license applies only to those patent claims licensable
    by such Contributor that are necessarily infringed by their
    Contribution(s) alone or by combination of their Contribution(s)
    with the Work to which such Contribution(s) was submitted. If You
    institute patent litigation against any entity (including a
    cross-claim or counterclaim in a lawsuit) alleging that the Work
    or a Contribution incorporated within the Work constitutes direct
    or contributory patent infringement, then any patent licenses
    granted to You under this License for that Work shall terminate
    as of the date such litigation is filed.

4.  Redistribution. You may reproduce and distribute copies of the
    Work or Derivative Works thereof in any medium, with or without
    modifications, and in Source or Object form, provided that You
    meet the following conditions:

    (a) You must give any other recipients of the Work or
    Derivative Works a copy of this License; and

    (b) You must cause any modified files to carry prominent notices
    stating that You changed the files; and

    (c) You must retain, in the Source form of any Derivative Works
    that You distribute, all copyright, patent, trademark, and
    attribution notices from the Source form of the Work,
    excluding those notices that do not pertain to any part of
    the Derivative Works; and

    (d) If the Work includes a "NOTICE" text file as part of its
    distribution, then any Derivative Works that You distribute must
    include a readable copy of the attribution notices contained
    within such NOTICE file, excluding those notices that do not
    pertain to any part of the Derivative Works, in at least one
    of the following places: within a NOTICE text file distributed
    as part of the Derivative Works; within the Source form or
    documentation, if provided along with the Derivative Works; or,
    within a display generated by the Derivative Works, if and
    wherever such third-party notices normally appear. The contents
    of the NOTICE file are for informational purposes only and
    do not modify the License. You may add Your own attribution
    notices within Derivative Works that You distribute, alongside
    or as an addendum to the NOTICE text from the Work, provided
    that such additional attribution notices cannot be construed
    as modifying the License.

    You may add Your own copyright statement to Your modifications and
    may provide additional or different license terms and conditions
    for use, reproduction, or distribution of Your modifications, or
    for any such Derivative Works as a whole, provided Your use,
    reproduction, and distribution of the Work otherwise complies with
    the conditions stated in this License.

5.  Submission of Contributions. Unless You explicitly state otherwise,
    any Contribution intentionally submitted for inclusion in the Work
    by You to the Licensor shall be under the terms and conditions of
    this License, without any additional terms or conditions.
    Notwithstanding the above, nothing herein shall supersede or modify
    the terms of any separate license agreement you may have executed
    with Licensor regarding such Contributions.

6.  Trademarks. This License does not grant permission to use the trade
    names, trademarks, service marks, or product names of the Licensor,
    except as required for reasonable and customary use in describing the
    origin of the Work and reproducing the content of the NOTICE file.

7.  Disclaimer of Warranty. Unless required by applicable law or
    agreed to in writing, Licensor provides the Work (and each
    Contributor provides its Contributions) on an "AS IS" BASIS,
    WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
    implied, including, without limitation, any warranties or conditions
    of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
    PARTICULAR PURPOSE. You are solely responsible for determining the
    appropriateness of using or redistributing the Work and assume any
    risks associated with Your exercise of permissions under this License.

8.  Limitation of Liability. In no event and under no legal theory,
    whether in tort (including negligence), contract, or otherwise,
    unless required by applicable law (such as deliberate and grossly
    negligent acts) or agreed to in writing, shall any Contributor be
    liable to You for damages, including any direct, indirect, special,
    incidental, or consequential damages of any character arising as a
    result of this License or out of the use or inability to use the
    Work (including but not limited to damages for loss of goodwill,
    work stoppage, computer failure or malfunction, or any and all
    other commercial damages or losses), even if such Contributor
    has been advised of the possibility of such damages.

9.  Accepting Warranty or Additional Liability. While redistributing
    the Work or Derivative Works thereof, You may choose to offer,
    and charge a fee for, acceptance of support, warranty, indemnity,
    or other liability obligations and/or rights consistent with this
    License. However, in accepting such obligations, You may act only
    on Your own behalf and on Your sole responsibility, not on behalf
    of any other Contributor, and only if You agree to indemnify,
    defend, and hold each Contributor harmless for any liability
    incurred by, or claims asserted against, such Contributor by reason
    of your accepting any such warranty or additional liability.

END OF TERMS AND CONDITIONS

APPENDIX: How to apply the Apache License to your work.

      To apply the Apache License to your work, attach the following
      boilerplate notice, with the fields enclosed by brackets "[]"
      replaced with your own identifying information. (Don't include
      the brackets!)  The text should be enclosed in the appropriate
      comment syntax for the file format. We also recommend that a
      file or class name and description of purpose be included on the
      same "printed page" as the copyright notice for easier
      identification within third-party archives.

Copyright 2025 OpenAI

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.

## WebRTC for iOS voice

The iOS app links the WebRTC XCFramework from the `stasel/WebRTC` Swift package,
pinned to 150.0.0 (binary SHA-256
`f9890492b0016e4c88ab20f07867b8b420054caedc8a692b2ec6ac041f3cf6b2`), a build of
Google's WebRTC: https://github.com/stasel/WebRTC/tree/150.0.0. WebRTC is
licensed under the BSD 3-Clause License, "Copyright (c) 2011, The WebRTC project
authors. All rights reserved.", with Google's additional patent grant:
https://webrtc.org/support/license. The full license text ships in the iOS app
as `Voice/WebRTC-LICENSE.txt`. The binary also contains WebRTC's own bundled
third-party components (such as BoringSSL, libsrtp, Opus, libyuv and Abseil)
under their respective licenses; see the WebRTC source tree's `third_party/`
for each license.
