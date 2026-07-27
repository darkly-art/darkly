<div align="center">

<a href="https://darkly.art"><img src="https://github.com/user-attachments/assets/62115b89-ab63-453c-93ce-a513e500fad7" alt="Darkly — a GPU-native paint engine in Rust" width="600"></a>

**Proc-macro support for [`darkly`](https://crates.io/crates/darkly), the GPU-native paint engine in Rust: the `#[handlers]` engine-bridge macro that turns tagged engine methods into the request/response protocol surface.**

[![crates.io](https://img.shields.io/crates/v/darkly-macros?style=for-the-badge&logo=rust&label=crates.io&labelColor=black&color=9500ff)](https://crates.io/crates/darkly-macros)
[![docs.rs](https://img.shields.io/docsrs/darkly-macros?style=for-the-badge&logo=docsdotrs&labelColor=black&color=8100ff)](https://docs.rs/darkly-macros)
[![GitHub](https://img.shields.io/github/stars/darkly-art/darkly?style=for-the-badge&logo=github&label=GitHub&labelColor=black&color=6c00ff)](https://github.com/darkly-art/darkly)
[![License](https://img.shields.io/crates/l/darkly-macros?style=for-the-badge&label=License&labelColor=black&color=5800ff)](https://github.com/darkly-art/darkly/blob/master/LICENSE)
[![Discord](https://img.shields.io/discord/1495886270780539021?style=for-the-badge&logo=discord&logoColor=white&label=Discord&labelColor=black&color=4400ff)](https://discord.gg/kFz2FGhbpu)

</div>

---

`darkly-macros` is an internal implementation detail of [Darkly](https://darkly.art), a GPU-native paint program for artists. It provides the `#[handlers]` procedural macro used by the [`darkly`](https://crates.io/crates/darkly) core crate to derive its request/response protocol surface.

Tag an `impl DarklyEngine { … }` block with `#[handlers]` and mark the methods that should be reachable over the protocol with an inner `#[handler]`. For each marked method the macro derives — from the signature alone — the `Deserialize` request struct and the registration that decodes it, calls the method, and encodes the response. The method name is the protocol kind; the signature is the single source of truth.

You almost certainly want the [`darkly`](https://crates.io/crates/darkly) crate, not this one directly. This crate is published so that `darkly` can depend on it from crates.io.

## Project

- **Try it now:** [demo.darkly.art](https://demo.darkly.art)
- **Documentation:** [darkly.art/docs](https://darkly.art/docs)
- **Community:** [Discord](https://discord.gg/kFz2FGhbpu)
- **Issues and source:** [github.com/darkly-art/darkly](https://github.com/darkly-art/darkly)

## Acknowledgements

A special thank you to **[Nick Cameron](https://github.com/nrc)** for graciously gifting us the `darkly` and `darkly-macros` crates on crates.io. 💜
