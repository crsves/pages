use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, ValueEnum};
use pages_cli::{doc, iwa, render};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

/// Render Apple Pages documents in the terminal.
#[derive(Parser)]
#[command(name = "pages", version)]
struct Cli {
    /// .pages file(s) to render
    #[arg(required_unless_present_any = ["generate_completions", "generate_man"])]
    files: Vec<PathBuf>,

    /// Show output in a pager ($PAGER, default `less -R`)
    #[arg(short, long)]
    pager: bool,

    /// Wrap width in columns (default: terminal width, capped at 100)
    #[arg(short, long)]
    width: Option<usize>,

    /// When to use colours and text styles
    #[arg(long, value_enum, default_value_t = ColorMode::Auto)]
    color: ColorMode,

    /// Print a shell completion script (for packagers)
    #[arg(long, value_name = "SHELL", hide = true, exclusive = true)]
    generate_completions: Option<clap_complete::Shell>,

    /// Print the man page in roff format (for packagers)
    #[arg(long, hide = true, exclusive = true)]
    generate_man: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum ColorMode {
    Auto,
    Always,
    Never,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pages: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<()> {
    if let Some(shell) = cli.generate_completions {
        clap_complete::generate(shell, &mut Cli::command(), "pages", &mut std::io::stdout());
        return Ok(());
    }
    if cli.generate_man {
        return clap_mangen::Man::new(Cli::command()).render(&mut std::io::stdout()).context("writing man page");
    }
    let tty = std::io::stdout().is_terminal();
    let color = match cli.color {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => tty && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()),
    };
    let width =
        cli.width.unwrap_or_else(|| terminal_size::terminal_size().map_or(80, |(w, _)| (w.0 as usize).min(100)));
    let opts = render::Options { width, color, hyperlinks: color && !cli.pager };

    let mut out = String::new();
    for (i, path) in cli.files.iter().enumerate() {
        let store = iwa::Store::open(path).with_context(|| path.display().to_string())?;
        let document = doc::load(&store);
        if cli.files.len() > 1 {
            if i > 0 {
                out.push('\n');
            }
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            out.push_str(&if color {
                format!("\x1b[2m── {name} ──\x1b[0m\n")
            } else {
                format!("── {name} ──\n")
            });
        }
        out.push_str(&render::render(&document, &opts));
    }

    if cli.pager && tty {
        page(&out)
    } else {
        let mut stdout = std::io::stdout().lock();
        match stdout.write_all(out.as_bytes()) {
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
            r => r.context("writing output"),
        }
    }
}

fn page(text: &str) -> Result<()> {
    let pager = std::env::var("PAGER").ok().filter(|p| !p.trim().is_empty());
    let mut cmd = match &pager {
        Some(p) => {
            let mut c = Command::new("sh");
            c.args(["-c", p]);
            c
        }
        None => {
            let mut c = Command::new("less");
            c.arg("-R");
            c
        }
    };
    let mut child = match cmd.stdin(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(_) => {
            print!("{text}");
            return Ok(());
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        // The user quitting the pager early closes the pipe; that's fine.
        let _ = stdin.write_all(text.as_bytes());
    }
    child.wait().context("waiting for pager")?;
    Ok(())
}
