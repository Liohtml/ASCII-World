use anyhow::{Context, Result};
use ascii_world::paint::Background;
use ascii_world::{charset, engine, mcp, paint, render};
use clap::{Parser, Subcommand, ValueEnum};
use std::io::{Read, Write};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "ascii-world",
    version,
    about = "Blazing-fast image → ASCII engine built for agent workflows",
    after_help = "EXAMPLES:\n  \
        ascii-world txt photo.jpg --width 120\n  \
        ascii-world txt photo.jpg --ansi                # true-color terminal art\n  \
        ascii-world txt photo.jpg --json | jq .lines    # machine-readable\n  \
        cat photo.jpg | ascii-world txt - --width 80    # streams via stdin\n  \
        ascii-world img photo.jpg -o art.png --color    # rendered PNG\n  \
        ascii-world mcp                                 # MCP server for agents"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Convert an image to ASCII text (plain, ANSI color, or JSON)
    Txt {
        /// Input image path, or '-' to read from stdin
        input: String,
        /// Output width in characters
        #[arg(short, long, default_value_t = 100)]
        width: u32,
        /// Character set: built-in name or 'custom:<chars dark→light>'
        #[arg(short, long, default_value = "complex")]
        charset: String,
        /// Invert brightness mapping (for light terminal themes)
        #[arg(long)]
        invert: bool,
        /// Emit 24-bit ANSI colors
        #[arg(long, conflicts_with = "json")]
        ansi: bool,
        /// Emit structured JSON (lines, charset, per-cell colors)
        #[arg(long)]
        json: bool,
        /// Write to a file instead of stdout
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Convert an image to a rendered ASCII-art image (PNG)
    Img {
        /// Input image path, or '-' to read from stdin
        input: String,
        /// Output image path
        #[arg(short, long)]
        output: PathBuf,
        /// Output width in characters
        #[arg(short, long, default_value_t = 200)]
        width: u32,
        /// Character set: built-in name or 'custom:<chars dark→light>'
        #[arg(short, long, default_value = "complex")]
        charset: String,
        /// Paint each glyph with the cell's average source color
        #[arg(long)]
        color: bool,
        /// Background color
        #[arg(long, value_enum, default_value_t = Bg::Black)]
        bg: Bg,
        /// Font size in pixels (controls output resolution)
        #[arg(long, default_value_t = 16.0)]
        font_px: f32,
        /// Invert brightness mapping. Default: automatic — inverted on black
        /// backgrounds so glyph density tracks contrast against the canvas.
        /// Pass --invert true/false to force either mapping.
        #[arg(long, num_args = 0..=1, default_missing_value = "true")]
        invert: Option<bool>,
    },
    /// List built-in character sets
    Charsets,
    /// Run as an MCP server on stdio (for Claude Code, Cursor, custom agents)
    Mcp,
}

#[derive(Clone, Copy, ValueEnum)]
enum Bg {
    Black,
    White,
}

impl From<Bg> for Background {
    fn from(bg: Bg) -> Self {
        match bg {
            Bg::Black => Background::Black,
            Bg::White => Background::White,
        }
    }
}

fn load_image(input: &str) -> Result<image::RgbImage> {
    let img = if input == "-" {
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .context("failed to read image from stdin")?;
        image::load_from_memory(&buf).context("stdin is not a supported image format")?
    } else {
        image::open(input).with_context(|| format!("failed to open image '{input}'"))?
    };
    Ok(img.to_rgb8())
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Txt {
            input,
            width,
            charset,
            invert,
            ansi,
            json,
            output,
        } => {
            let ramp = charset::resolve(&charset)?;
            // On a white PNG/terminal the ramp reads inverted; txt defaults
            // assume dark terminals, so --invert is explicit user choice.
            let grid = engine::convert(
                &load_image(&input)?,
                &engine::Options {
                    width,
                    charset: ramp.clone(),
                    invert,
                    aspect: 2.0,
                },
            )?;
            if grid.cols < width {
                eprintln!(
                    "note: --width {width} clamped to image width ({} columns)",
                    grid.cols
                );
            }
            let rendered = if json {
                render::to_json(&grid, &render::effective_ramp(&ramp, invert), true)
            } else if ansi {
                render::to_ansi(&grid)
            } else {
                render::to_text(&grid)
            };
            match output {
                Some(path) => std::fs::write(&path, rendered)
                    .with_context(|| format!("failed to write {}", path.display()))?,
                None => {
                    std::io::stdout().write_all(rendered.as_bytes())?;
                }
            }
        }
        Command::Img {
            input,
            output,
            width,
            charset,
            color,
            bg,
            font_px,
            invert,
        } => {
            let ramp = charset::resolve(&charset)?;
            let background: Background = bg.into();
            let invert = invert.unwrap_or(background.default_invert());
            let grid = engine::convert(
                &load_image(&input)?,
                &engine::Options {
                    width,
                    charset: ramp,
                    invert,
                    aspect: 2.0,
                },
            )?;
            let img = paint::paint_png(&grid, background, color, font_px)?;
            img.save(&output)
                .with_context(|| format!("failed to write {}", output.display()))?;
            eprintln!(
                "wrote {} ({}x{} chars, {}x{} px)",
                output.display(),
                grid.cols,
                grid.rows,
                img.width(),
                img.height()
            );
        }
        Command::Charsets => {
            for name in charset::NAMED {
                let ramp: String = charset::resolve(name)?.iter().collect();
                println!("{name:<12} {ramp}");
            }
        }
        Command::Mcp => mcp::serve()?,
    }
    Ok(())
}
