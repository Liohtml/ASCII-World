use anyhow::{Context, Result};
use ascii_world::charset::Charset;
use ascii_world::font::FontStack;
use ascii_world::input::{self, Frame};
use ascii_world::paint::{Background, PaintOptions};
use ascii_world::{anim, charset, engine, mcp, paint, render, video};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "ascii-world",
    version,
    about = "Blazing-fast image → ASCII engine built for agent workflows",
    after_help = "EXAMPLES:\n  \
        ascii-world txt photo.jpg --width 120\n  \
        ascii-world txt photo.jpg --ansi                # true-color terminal art\n  \
        ascii-world txt photo.jpg --charset braille     # 2×4 dots per cell\n  \
        ascii-world txt photo.jpg --json | jq .lines    # machine-readable\n  \
        cat photo.jpg | ascii-world txt - --width 80    # streams via stdin\n  \
        ascii-world img logo.svg -o art.png --bg transparent --color\n  \
        ascii-world img clip.gif -o art.gif --color     # animated\n  \
        ascii-world video clip.mp4 -o art.mp4 --color   # needs ffmpeg\n  \
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
        #[command(flatten)]
        common: CommonArgs,
        #[command(flatten)]
        text: TxtArgs,
    },
    /// Convert an image to a rendered ASCII-art image (PNG, or animated GIF)
    Img {
        /// Input image path, or '-' to read from stdin
        input: String,
        /// Output image path (.png, or .gif for animation)
        #[arg(short, long)]
        output: PathBuf,
        /// Output width in characters
        #[arg(short, long, default_value_t = 200)]
        width: u32,
        #[command(flatten)]
        common: CommonArgs,
        #[command(flatten)]
        render: RenderArgs,
    },
    /// Convert a video to an ASCII-art video (needs ffmpeg on PATH)
    Video {
        /// Input video path
        input: PathBuf,
        /// Output video path (.mp4, .gif, ... — the extension picks the codec)
        #[arg(short, long)]
        output: PathBuf,
        /// Output width in characters
        #[arg(short, long, default_value_t = 100)]
        width: u32,
        #[command(flatten)]
        common: CommonArgs,
        #[command(flatten)]
        render: RenderArgs,
        /// Output frame rate (default: the source's)
        #[arg(long)]
        fps: Option<f64>,
        /// Inset the source video in the corner at this fraction of the width
        #[arg(long, default_value_t = 0.0, value_name = "RATIO")]
        overlay: f32,
        /// H.264 quality, lower is better (ignored for GIF output)
        #[arg(long, default_value_t = 20)]
        crf: u8,
    },
    /// List built-in character sets
    Charsets,
    /// Run as an MCP server on stdio (for Claude Code, Cursor, custom agents)
    Mcp,
}

/// Options every conversion accepts.
#[derive(Args, Clone)]
struct CommonArgs {
    /// Character set: built-in name, 'braille', or 'custom:<chars dark→light>'
    #[arg(short, long, default_value = "complex")]
    charset: String,
    /// Cells whose mean alpha is below this render blank (0–255)
    #[arg(long, default_value_t = engine::DEFAULT_ALPHA_THRESHOLD, value_name = "ALPHA")]
    alpha_threshold: u8,
    /// Braille dot cutoff (0–255); by default one is picked per image
    #[arg(long, value_name = "LUMA")]
    threshold: Option<u8>,
    /// Extra font file (TTF/OTF), tried before the built-in fonts
    #[arg(long, value_name = "PATH")]
    font: Option<PathBuf>,
}

/// Options only the text renderer accepts.
#[derive(Args, Clone)]
struct TxtArgs {
    /// Invert brightness mapping (for light terminal themes)
    #[arg(long)]
    invert: bool,
    /// Cell height ÷ width; 2.0 matches a typical terminal
    #[arg(long, default_value_t = 2.0)]
    aspect: f32,
    /// Emit 24-bit ANSI colors
    #[arg(long, conflicts_with = "json")]
    ansi: bool,
    /// Emit structured JSON (lines, charset, per-cell colors, alpha)
    #[arg(long)]
    json: bool,
    /// Write to a file instead of stdout
    #[arg(short, long)]
    output: Option<PathBuf>,
}

/// Options shared by the image and video renderers.
#[derive(Args, Clone)]
struct RenderArgs {
    /// Paint each glyph with the cell's average source color
    #[arg(long)]
    color: bool,
    /// Background color
    #[arg(long, value_enum, default_value_t = Bg::Black)]
    bg: Bg,
    /// Font size in pixels (controls output resolution)
    #[arg(long, default_value_t = 16.0)]
    font_px: f32,
    /// Invert brightness mapping. Default: automatic — inverted on dark
    /// backgrounds so glyph density tracks contrast against the canvas.
    /// Pass --invert true/false to force either mapping.
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    invert: Option<bool>,
    /// Keep blank margins instead of trimming to the drawn glyphs
    #[arg(long)]
    no_crop: bool,
    /// Cell height ÷ width (default: measured from the font)
    #[arg(long)]
    aspect: Option<f32>,
}

#[derive(Clone, Copy, ValueEnum)]
enum Bg {
    Black,
    White,
    /// Only glyphs carry opacity — needs a format with alpha (PNG/GIF)
    Transparent,
}

impl From<Bg> for Background {
    fn from(bg: Bg) -> Self {
        match bg {
            Bg::Black => Background::Black,
            Bg::White => Background::White,
            Bg::Transparent => Background::Transparent,
        }
    }
}

impl CommonArgs {
    fn fonts(&self) -> Result<FontStack> {
        match &self.font {
            Some(path) => FontStack::with_font_file(path),
            None => Ok(FontStack::embedded()),
        }
    }
}

/// Everything the two rendering paths need, resolved once.
struct Renderer {
    fonts: FontStack,
    engine: engine::Options,
    paint: PaintOptions,
}

impl Renderer {
    fn new(common: &CommonArgs, render: &RenderArgs, width: u32) -> Result<Self> {
        let fonts = common.fonts()?;
        let charset = charset::resolve_with(&common.charset, &fonts)?;
        let background: Background = render.bg.into();
        let paint = PaintOptions::new(&fonts, &charset, render.font_px)?
            .background(background)
            .colored(render.color)
            .crop(!render.no_crop);
        // Cells are not square, and how far from square depends on the font —
        // measuring keeps the picture's proportions instead of assuming 2.0.
        let aspect = render.aspect.unwrap_or_else(|| paint.metrics.aspect());
        Ok(Self {
            engine: engine::Options {
                width,
                charset,
                invert: render.invert.unwrap_or(background.default_invert()),
                aspect,
                alpha_threshold: common.alpha_threshold,
                matte: background.matte(),
                braille_threshold: common.threshold,
            },
            fonts,
            paint,
        })
    }
}

fn note_clamped_width(cols: u32, requested: u32) {
    if cols < requested {
        eprintln!("note: --width {requested} clamped to image width ({cols} columns)");
    }
}

fn run_txt(input: String, width: u32, common: CommonArgs, text: TxtArgs) -> Result<()> {
    let TxtArgs {
        invert,
        aspect,
        ansi,
        json,
        output,
    } = text;
    let fonts = common.fonts()?;
    let charset = charset::resolve_with(&common.charset, &fonts)?;
    let bytes = input::read_bytes(&input)?;
    let image = input::decode_still(&bytes, input::svg_target_px(width))?;
    // txt defaults assume a dark terminal, where a blank cell reads as the
    // dark end of the ramp; --invert flips which surface we composite onto.
    let matte = Some(if invert { [0, 0, 0] } else { [255, 255, 255] });
    let grid = engine::convert(
        &image,
        &engine::Options {
            width,
            charset: charset.clone(),
            invert,
            aspect,
            alpha_threshold: common.alpha_threshold,
            matte,
            braille_threshold: common.threshold,
        },
    )?;
    note_clamped_width(grid.cols, width);

    let rendered = if json {
        render::to_json(&grid, &charset, invert, true)
    } else if ansi {
        render::to_ansi(&grid)
    } else {
        render::to_text(&grid)
    };
    match output {
        Some(path) => std::fs::write(&path, rendered)
            .with_context(|| format!("failed to write {}", path.display()))?,
        None => std::io::stdout().write_all(rendered.as_bytes())?,
    }
    Ok(())
}

fn run_img(
    input: String,
    output: PathBuf,
    width: u32,
    common: CommonArgs,
    render_args: RenderArgs,
) -> Result<()> {
    let r = Renderer::new(&common, &render_args, width)?;
    let bytes = input::read_bytes(&input)?;
    let mut frames = input::decode_frames(&bytes, input::svg_target_px(width))?;
    let animated = frames.len() > 1;

    if animated && !anim::wants_gif(&output) {
        eprintln!(
            "note: '{input}' has {} frames; writing frame 1 only (use a .gif output to animate)",
            frames.len()
        );
        frames.truncate(1);
    }

    if frames.len() == 1 {
        let grid = engine::convert(&frames[0].image, &r.engine)?;
        note_clamped_width(grid.cols, width);
        let img = paint::paint_png(&grid, &r.fonts, &r.paint)?;
        save_still(&img, &output)?;
        eprintln!(
            "wrote {} ({}x{} chars, {}x{} px)",
            output.display(),
            grid.cols,
            grid.rows,
            img.width(),
            img.height()
        );
        return Ok(());
    }

    // Animated: every frame must land on the same canvas, so the crop box is
    // measured once and reused.
    let mut painted: Vec<Frame> = Vec::with_capacity(frames.len());
    let mut bounds = None;
    let mut cols = 0;
    for (i, frame) in frames.iter().enumerate() {
        let grid = engine::convert(&frame.image, &r.engine)?;
        cols = grid.cols;
        let mut canvas = paint::paint_canvas(&grid, &r.fonts, &r.paint)?;
        if r.paint.crop {
            if bounds.is_none() {
                bounds = paint::content_bounds(&canvas, r.paint.background);
            }
            if let Some(b) = bounds {
                canvas = paint::crop_to(&canvas, b);
            }
        }
        painted.push(Frame {
            image: canvas,
            delay_ms: frame.delay_ms,
        });
        eprint!("\rframe {}/{}", i + 1, frames.len());
    }
    eprintln!();
    note_clamped_width(cols, width);
    anim::write_gif(&output, &painted)?;
    let (w, h) = painted[0].image.dimensions();
    eprintln!(
        "wrote {} ({} frames, {w}x{h} px)",
        output.display(),
        painted.len()
    );
    Ok(())
}

/// Save a still, dropping the alpha channel for formats that lack one.
fn save_still(img: &image::RgbaImage, output: &Path) -> Result<()> {
    let keeps_alpha = matches!(
        output
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png") | Some("gif") | Some("webp") | None
    );
    let result = if keeps_alpha {
        img.save(output)
    } else {
        image::DynamicImage::ImageRgba8(img.clone())
            .to_rgb8()
            .save(output)
    };
    result.with_context(|| format!("failed to write {}", output.display()))
}

fn run_video(
    input: PathBuf,
    output: PathBuf,
    width: u32,
    common: CommonArgs,
    render_args: RenderArgs,
    opts: video::VideoOptions,
) -> Result<()> {
    let r = Renderer::new(&common, &render_args, width)?;
    let stats = video::convert_video(
        &input,
        &output,
        &r.fonts,
        &r.engine,
        &r.paint,
        &opts,
        |frames| {
            if frames % 12 == 0 {
                eprint!("\rframe {frames}");
            }
        },
    )?;
    eprintln!(
        "\rwrote {} ({} frames, {}x{} px)",
        output.display(),
        stats.frames,
        stats.width,
        stats.height
    );
    Ok(())
}

fn run_charsets() -> Result<()> {
    for name in charset::NAMED {
        let sample: String = match charset::resolve(name)? {
            Charset::Ramp(ramp) => ramp.iter().collect(),
            // Braille composes 256 patterns from dots; show the density ramp
            // a run of increasingly filled cells produces.
            Charset::Braille => "⣿⡿⠿⠟⠛⠋⠉⠁⠀".into(),
        };
        println!("{name:<12} {sample}");
    }
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Txt {
            input,
            width,
            common,
            text,
        } => run_txt(input, width, common, text)?,
        Command::Img {
            input,
            output,
            width,
            common,
            render,
        } => run_img(input, output, width, common, render)?,
        Command::Video {
            input,
            output,
            width,
            common,
            render,
            fps,
            overlay,
            crf,
        } => run_video(
            input,
            output,
            width,
            common,
            render,
            video::VideoOptions { fps, overlay, crf },
        )?,
        Command::Charsets => run_charsets()?,
        Command::Mcp => mcp::serve()?,
    }
    Ok(())
}
