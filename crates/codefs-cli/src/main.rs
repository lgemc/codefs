//! CodeFS CLI - Mount Python code as a virtual filesystem organized by class and method.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use python_parser::PythonParser;
use python_vfs::{PythonFuseFs, PythonVfsBuilder};
use std::path::PathBuf;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser)]
#[command(name = "codefs")]
#[command(author, version, about = "Mount Python code as a virtual filesystem", long_about = None)]
struct Cli {
    /// Enable verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Mount a Python directory as a virtual filesystem
    Mount {
        /// Path to the Python source directory
        #[arg(short, long)]
        source: PathBuf,

        /// Path to mount the virtual filesystem
        #[arg(short, long)]
        mountpoint: PathBuf,

        /// Don't include module-level functions
        #[arg(long)]
        no_functions: bool,

        /// Don't include metadata files
        #[arg(long)]
        no_metadata: bool,

        /// Organize flat (classes at top level) instead of grouped by file
        #[arg(long)]
        flat: bool,

        /// Run in foreground (don't daemonize)
        #[arg(short, long)]
        foreground: bool,
    },

    /// Parse a Python directory and print the structure
    Parse {
        /// Path to the Python source directory
        #[arg(short, long)]
        source: PathBuf,

        /// Output format (text, json)
        #[arg(short, long, default_value = "text")]
        format: OutputFormat,
    },

    /// Show the virtual filesystem tree structure
    Tree {
        /// Path to the Python source directory
        #[arg(short, long)]
        source: PathBuf,

        /// Maximum depth to display
        #[arg(short, long)]
        depth: Option<usize>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputFormat {
    Text,
    Json,
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            _ => Err(format!("Unknown format: {}", s)),
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Set up logging
    let level = match cli.verbose {
        0 => Level::WARN,
        1 => Level::INFO,
        2 => Level::DEBUG,
        _ => Level::TRACE,
    };

    let subscriber = FmtSubscriber::builder()
        .with_max_level(level)
        .with_target(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .context("Failed to set up logging")?;

    match cli.command {
        Commands::Mount {
            source,
            mountpoint,
            no_functions,
            no_metadata,
            flat,
            foreground,
        } => {
            cmd_mount(
                source,
                mountpoint,
                !no_functions,
                !no_metadata,
                !flat,
                foreground,
            )?;
        }
        Commands::Parse { source, format } => {
            cmd_parse(source, format)?;
        }
        Commands::Tree { source, depth } => {
            cmd_tree(source, depth)?;
        }
    }

    Ok(())
}

fn cmd_mount(
    source: PathBuf,
    mountpoint: PathBuf,
    include_functions: bool,
    include_metadata: bool,
    group_by_file: bool,
    foreground: bool,
) -> Result<()> {
    info!("Parsing Python source directory: {}", source.display());

    let builder = PythonVfsBuilder::new()
        .include_functions(include_functions)
        .include_metadata(include_metadata)
        .group_by_file(group_by_file);

    let tree = builder
        .build_from_directory(&source)
        .context("Failed to build VFS tree")?;

    info!("Mounting at: {}", mountpoint.display());

    // Ensure mountpoint exists
    if !mountpoint.exists() {
        std::fs::create_dir_all(&mountpoint)
            .context("Failed to create mountpoint directory")?;
    }

    let fs = PythonFuseFs::new(tree);

    if foreground {
        println!("Mounting {} at {} (foreground mode)", source.display(), mountpoint.display());
        println!("Press Ctrl+C to unmount");
        fs.mount_foreground(&mountpoint)
            .context("Failed to mount filesystem")?;
    } else {
        fs.mount(&mountpoint)
            .context("Failed to mount filesystem")?;
    }

    Ok(())
}

fn cmd_parse(source: PathBuf, format: OutputFormat) -> Result<()> {
    let mut parser = PythonParser::new()
        .context("Failed to create parser")?;

    let parsed = parser
        .parse_directory(&source)
        .context("Failed to parse directory")?;

    match format {
        OutputFormat::Text => {
            println!("Parsed {} files from {}", parsed.files.len(), source.display());
            println!();

            for (path, file) in &parsed.files {
                println!("File: {}", path.display());
                println!("  Module: {}", file.module.name);

                if let Some(doc) = &file.module.docstring {
                    println!("  Docstring: {}", doc.lines().next().unwrap_or(""));
                }

                if !file.module.functions.is_empty() {
                    println!("  Functions:");
                    for func in &file.module.functions {
                        let async_prefix = if func.is_async { "async " } else { "" };
                        println!("    - {}{}()", async_prefix, func.name);
                    }
                }

                if !file.module.classes.is_empty() {
                    println!("  Classes:");
                    for class in &file.module.classes {
                        let bases = if class.bases.is_empty() {
                            String::new()
                        } else {
                            format!("({})", class.bases.join(", "))
                        };
                        println!("    - {}{}", class.name, bases);

                        for method in &class.methods {
                            let decorators: Vec<_> = method
                                .decorators
                                .iter()
                                .map(|d| format!("@{}", d.name))
                                .collect();
                            let dec_str = if decorators.is_empty() {
                                String::new()
                            } else {
                                format!(" {}", decorators.join(" "))
                            };
                            let async_prefix = if method.is_async { "async " } else { "" };
                            println!("      - {}{}(){}", async_prefix, method.name, dec_str);
                        }
                    }
                }
                println!();
            }
        }
        OutputFormat::Json => {
            let output: Vec<_> = parsed
                .files
                .iter()
                .map(|(path, file)| {
                    serde_json::json!({
                        "path": path.display().to_string(),
                        "module": file.module.name,
                        "docstring": file.module.docstring,
                        "functions": file.module.functions.iter().map(|f| {
                            serde_json::json!({
                                "name": f.name,
                                "is_async": f.is_async,
                                "docstring": f.docstring,
                            })
                        }).collect::<Vec<_>>(),
                        "classes": file.module.classes.iter().map(|c| {
                            serde_json::json!({
                                "name": c.name,
                                "bases": c.bases,
                                "docstring": c.docstring,
                                "methods": c.methods.iter().map(|m| {
                                    serde_json::json!({
                                        "name": m.name,
                                        "is_async": m.is_async,
                                        "is_staticmethod": m.is_staticmethod,
                                        "is_classmethod": m.is_classmethod,
                                        "is_property": m.is_property,
                                        "docstring": m.docstring,
                                    })
                                }).collect::<Vec<_>>(),
                            })
                        }).collect::<Vec<_>>(),
                    })
                })
                .collect();

            println!("{}", serde_json::to_string_pretty(&output)?);
        }
    }

    Ok(())
}

fn cmd_tree(source: PathBuf, max_depth: Option<usize>) -> Result<()> {
    let builder = PythonVfsBuilder::new();
    let tree = builder
        .build_from_directory(&source)
        .context("Failed to build VFS tree")?;

    println!("Virtual filesystem tree for: {}", source.display());
    println!();

    print_tree_node(tree.root(), "", true, 0, max_depth);

    Ok(())
}

fn print_tree_node(
    node: &vfs_core::VfsNode,
    prefix: &str,
    is_last: bool,
    depth: usize,
    max_depth: Option<usize>,
) {
    if let Some(max) = max_depth {
        if depth > max {
            return;
        }
    }

    let connector = if depth == 0 {
        ""
    } else if is_last {
        "└── "
    } else {
        "├── "
    };

    let name = if depth == 0 { "/" } else { &node.name };
    let suffix = if node.is_dir() { "/" } else { "" };

    println!("{}{}{}{}", prefix, connector, name, suffix);

    if node.is_dir() {
        let new_prefix = if depth == 0 {
            String::new()
        } else if is_last {
            format!("{}    ", prefix)
        } else {
            format!("{}│   ", prefix)
        };

        let mut children: Vec<_> = node.children.values().collect();
        children.sort_by(|a, b| {
            // Directories first, then files, then alphabetically
            match (a.is_dir(), b.is_dir()) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.cmp(&b.name),
            }
        });

        for (i, child) in children.iter().enumerate() {
            print_tree_node(child, &new_prefix, i == children.len() - 1, depth + 1, max_depth);
        }
    }
}
