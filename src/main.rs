use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use vni::{format_utc, now, short_id, Author, Payload, Pixels, VniFile};

#[derive(Parser)]
#[command(name = "vni", version, about = "Version-controlled images: a base image plus authored, commit-like changes in one .vni file")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Args)]
struct Meta {
    /// Commit message
    #[arg(short, long)]
    message: String,
    /// Author as "Name <email>". Defaults to $VNI_AUTHOR, then `git config user.name/user.email`
    #[arg(short, long)]
    author: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new .vni file from a base image (png, jpg, gif, bmp, webp, tiff)
    Init {
        /// Source image used as the base
        image: PathBuf,
        /// Output file (default: the image path with a .vni extension)
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[command(flatten)]
        meta: Meta,
    },
    /// Record a new version of the image
    Commit {
        file: PathBuf,
        /// Image holding the new version
        image: PathBuf,
        /// Allow a commit identical to HEAD
        #[arg(long)]
        allow_empty: bool,
        #[command(flatten)]
        meta: Meta,
    },
    /// Show the history, newest first
    Log {
        file: PathBuf,
        /// One line per commit
        #[arg(long)]
        oneline: bool,
    },
    /// Show details of one revision
    Show {
        file: PathBuf,
        #[arg(default_value = "HEAD")]
        rev: String,
    },
    /// Write a revision out as a regular image (format from the output extension)
    Export {
        file: PathBuf,
        /// HEAD, HEAD~N, BASE, #N (0 = base) or an id prefix
        rev: String,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Compare two revisions (default: HEAD~1 vs HEAD)
    Diff {
        file: PathBuf,
        #[arg(default_value = "HEAD~1")]
        from: String,
        #[arg(default_value = "HEAD")]
        to: String,
        /// Also write a visualisation with changed pixels in red
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Rebuild every revision and check all hashes
    Verify { file: PathBuf },
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn git_config(key: &str) -> Option<String> {
    let out = Command::new("git").args(["config", "--get", key]).output().ok()?;
    let v = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !v.is_empty()).then_some(v)
}

fn resolve_author(explicit: Option<&str>) -> Result<Author> {
    if let Some(a) = explicit {
        return Ok(Author::parse(a)?);
    }
    if let Ok(a) = std::env::var("VNI_AUTHOR") {
        return Ok(Author::parse(&a)?);
    }
    match (git_config("user.name"), git_config("user.email")) {
        (Some(name), Some(email)) => Ok(Author { name, email }),
        _ => bail!("no author: pass --author \"Name <email>\", set VNI_AUTHOR, or configure git user.name/user.email"),
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Cmd::Init { image, output, meta } => {
            let output = output.unwrap_or_else(|| image.with_extension("vni"));
            let px = Pixels::load(&image).with_context(|| format!("reading {}", image.display()))?;
            let file = VniFile::create(&output, &px, resolve_author(meta.author.as_deref())?, meta.message, now())
                .with_context(|| format!("creating {}", output.display()))?;
            println!("Created {} ({}x{}) base {}", output.display(), px.width, px.height, short_id(&file.base().id));
        }
        Cmd::Commit { file, image, allow_empty, meta } => {
            let mut f = VniFile::open(&file)?;
            let px = Pixels::load(&image).with_context(|| format!("reading {}", image.display()))?;
            let author = resolve_author(meta.author.as_deref())?;
            let c = f.commit(&px, author, meta.message, now(), allow_empty)?;
            let kind = match c.payload {
                Payload::Full(_) => "snapshot",
                Payload::Delta(_) => "delta",
            };
            println!("[{}] {} ({}, {} bytes)", short_id(&c.id), c.message.lines().next().unwrap_or(""), kind, c.payload.bytes().len());
        }
        Cmd::Log { file, oneline } => {
            let f = VniFile::open(&file)?;
            for (i, c) in f.commits().iter().enumerate().rev() {
                if oneline {
                    println!("{} #{} {} ({})", short_id(&c.id), i, c.message.lines().next().unwrap_or(""), c.author.name);
                } else {
                    println!("commit {} (#{}{})", c.hex_id(), i, if i == 0 { ", base" } else { "" });
                    println!("Author: {}", c.author);
                    println!("Date:   {}", format_utc(c.timestamp));
                    println!("Size:   {}x{}\n", c.width, c.height);
                    for line in c.message.lines() {
                        println!("    {line}");
                    }
                    println!();
                }
            }
        }
        Cmd::Show { file, rev } => {
            let f = VniFile::open(&file)?;
            let i = f.resolve(&rev)?;
            let c = &f.commits()[i];
            let (kind, bytes) = match &c.payload {
                Payload::Full(b) => ("snapshot", b.len()),
                Payload::Delta(b) => ("delta", b.len()),
            };
            println!("commit {} (#{})", c.hex_id(), i);
            println!("parent {}", c.parent.map(|p| vni::to_hex(&p)).unwrap_or_else(|| "none (base)".into()));
            println!("tree   {}", vni::to_hex(&c.tree));
            println!("Author: {}\nDate:   {}\nSize:   {}x{}\nStored: {} ({} bytes)\n", c.author, format_utc(c.timestamp), c.width, c.height, kind, bytes);
            for line in c.message.lines() {
                println!("    {line}");
            }
            if i > 0 {
                let (a, b) = (f.checkout(i - 1)?, f.checkout(i)?);
                if let Ok(s) = a.diff_stats(&b) {
                    println!("\n{} of {} pixels changed", s.changed_pixels, s.total_pixels);
                }
            }
        }
        Cmd::Export { file, rev, output } => {
            let f = VniFile::open(&file)?;
            let i = f.resolve(&rev)?;
            f.checkout(i)?.save(&output).with_context(|| format!("writing {}", output.display()))?;
            println!("Wrote {} ({})", output.display(), short_id(&f.commits()[i].id));
        }
        Cmd::Diff { file, from, to, output } => {
            let f = VniFile::open(&file)?;
            let (a, b) = (f.resolve(&from)?, f.resolve(&to)?);
            let (pa, pb) = (f.checkout(a)?, f.checkout(b)?);
            println!("{} -> {}", short_id(&f.commits()[a].id), short_id(&f.commits()[b].id));
            let s = pa.diff_stats(&pb)?;
            match s.bbox {
                None => println!("identical"),
                Some((x0, y0, x1, y1)) => {
                    println!("{} of {} pixels changed ({:.2}%)", s.changed_pixels, s.total_pixels, 100.0 * s.changed_pixels as f64 / s.total_pixels as f64);
                    println!("bounding box: ({x0},{y0}) to ({x1},{y1})");
                }
            }
            if let Some(out) = output {
                pa.diff_image(&pb)?.save(&out).with_context(|| format!("writing {}", out.display()))?;
                println!("Wrote {}", out.display());
            }
        }
        Cmd::Verify { file } => {
            let f = VniFile::open(&file)?;
            f.verify()?;
            println!("OK: {} commits verified", f.commits().len());
        }
    }
    Ok(())
}
