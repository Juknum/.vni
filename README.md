<img src="assets/icon.svg" width="64" height="64" alt="">

# vni

**V**ersioned **I**mage: a single `.vni` file that holds a base image plus a
history of changes, each with an author, date and message, like Git commits for
one picture.

- One file per image, no repository needed.
- Append-only: committing never rewrites earlier bytes.
- Compact: an edit stores a sparse pixel delta, so a small touch-up costs a few
  bytes, not a whole new image.
- Tamper-evident: commits are SHA-256 linked, and `vni verify` rebuilds every
  revision and checks it.
- Lossless round trip of every revision (stored as 8-bit RGBA).

## Usage

```sh
cargo install --path .        # or: cargo build --release

vni init logo.png -m "First draft" -a "Ada <ada@example.com>"   # -> logo.vni
vni commit logo.vni logo-v2.png -m "Fix colours" -a "Grace <grace@example.com>"

vni log logo.vni              # history, newest first (--oneline for short)
vni show logo.vni HEAD~1      # one revision, with stats vs its parent
vni diff logo.vni BASE HEAD -o diff.png   # changed pixels in red
vni export logo.vni #1 -o logo-first-edit.png
vni verify logo.vni
```

Revisions: `HEAD`, `HEAD~N`, `BASE`, `#N` (0 is the base) or a hex id prefix
of at least 4 characters.

Author comes from `--author "Name <email>"`, then `$VNI_AUTHOR`, then
`git config user.name` and `user.email`.

Inputs can be PNG, JPEG, GIF, BMP, WebP or TIFF. Exports use the extension of
the output path.

## Library

```rust
use vni::{Author, Pixels, VniFile};

let base = Pixels::load("logo.png")?;
let who = Author::parse("Ada <ada@example.com>")?;
let mut f = VniFile::create("logo.vni", &base, who.clone(), "First draft".into(), vni::now())?;
f.commit(&Pixels::load("logo-v2.png")?, who, "Fix colours".into(), vni::now(), false)?;
let first_edit = f.checkout(1)?;
```

The byte-level format, including the order of every field in the file and a
annotated hex dump, is in [SPEC.md](SPEC.md). A comparison with Git, how to host
many images on a shared remote, and the plan for branches and merges are in
[docs/git-comparison.md](docs/git-comparison.md).

## Limits

History is linear (no branches or merges yet). A whole file is read into memory
on open, and checking out a revision replays deltas from the nearest snapshot.
