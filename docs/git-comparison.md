# vni compared with Git

vni borrows Git's ideas (hash-linked commits, authors, messages) but applies
them to the pixels of a single image instead of the files of a tree.

## Side by side

| | Git | vni today |
|---|---|---|
| Unit of versioning | A tree of files | One image per `.vni` file |
| Where history lives | A `.git` directory | Inside the file itself |
| Commit identity | SHA-1 or SHA-256 of commit object | SHA-256 of the commit record |
| Change storage | Whole blobs, delta-packed by `git gc` | Sparse pixel delta, written at commit time |
| Unit of change | Line or byte, meaningful for text | Pixel, meaningful for images |
| Diff | Text diff | Changed-pixel count, bounding box, red overlay |
| Branches and merges | Yes | Not yet |
| Remote and sync | `fetch`, `push`, `pull` | None built in |
| Moving a single image | Copy the file and lose nothing | Copy the file and lose nothing |
| Working tree and index | Yes, with staging | No, you commit a whole image file |
| Rewriting history | `rebase`, `amend`, `reset` | No, append-only |

The main trade-off: a `.vni` file is self-contained and portable, but it cannot
share history with other files the way a Git repo shares objects.

## Hosting many images on a remote

vni has no network protocol of its own, so a remote is a Git repository (or any
file store) that holds one `.vni` file per image. Git carries the files, and vni
records the per-image history inside each one.

```sh
# One time: create the shared repository on your Git host, then
git clone git@host:team/artwork.git && cd artwork
printf '*.vni binary\n' > .gitattributes      # never text-merge or diff these
git add .gitattributes && git commit -m "Treat .vni as binary" && git push
```

Add an image:

```sh
vni init sources/hero.png -o hero.vni -m "First draft" -a "Ada <ada@example.com>"
git add hero.vni && git commit -m "Add hero" && git push
```

Edit an image. The vni commit is the authoritative, authored change. The Git
commit only publishes the file:

```sh
git pull
vni commit hero.vni hero-v2.png -m "Fix colours" -a "Grace <grace@example.com>"
vni verify hero.vni
git add hero.vni && git commit -m "hero: fix colours" && git push
```

Several images are simply several files, and each keeps an independent
history:

```
artwork/
  hero.vni
  logo.vni
  icons/play.vni
```

Practical notes:

- **Concurrent edits to one file conflict.** Because `.vni` is append-only and
  linear, two people who both appended to `hero.vni` produce two different
  files, and Git cannot combine them. Pull before you commit, or agree who
  owns a file. With Git LFS, `git lfs lock hero.vni` enforces this.
- **Large images.** Track `*.vni` with Git LFS (`git lfs track "*.vni"`) so
  clones stay small. The append-only layout helps packfile compression, but
  LFS is the safer choice for big files.
- **Different images never conflict**, since each lives in its own file.

## Branches and merges (proposed, not built)

Both fit the format without breaking it, because every commit already stores a
parent id. The design would be:

1. **Branches.** Add a second record type for named refs (`name -> commit id`),
   appended whenever a ref moves. The newest ref record for a name wins, so the
   file stays append-only. `HEAD` becomes the current branch rather than the
   last record.
2. **Two parents.** A merge commit lists a second parent. Format version 2
   would add an optional parent field, and v1 readers would reject the file
   cleanly through the version check.
3. **Pixel merge.** A three-way merge compares each pixel of both branches with
   their common ancestor. Pixels changed on only one side merge automatically.
   Pixels changed differently on both sides conflict, and the user picks a side
   per region or supplies a resolved image.
4. **Git integration.** With real history inside the file, a Git merge driver
   (`merge=vni` in `.gitattributes`) can combine two diverged copies of
   `hero.vni` by taking the union of their commits and merging the heads. That
   removes the concurrent-edit limitation above.
5. **Own remote.** Syncing two copies becomes "send the commits the other side
   lacks", which is cheap because ids are content hashes.

The CLI surface would be `vni branch`, `vni switch`, `vni merge` and
`vni log --graph`.
