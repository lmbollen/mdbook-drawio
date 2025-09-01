use anyhow::{Ok, Result};
use log::{debug, error};
use mdbook::book::{Book, BookItem, Chapter};
use mdbook::errors::Error;
use mdbook::preprocess::{Preprocessor, PreprocessorContext};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct DrawioPreprocessor;

impl Preprocessor for DrawioPreprocessor {
    fn name(&self) -> &str {
        "mdbook-drawio"
    }

    fn supports_renderer(&self, _renderer: &str) -> bool {
        true
    }

    fn run(&self, ctx: &PreprocessorContext, mut book: Book) -> Result<Book, Error> {
        fn process_item(ctx: &PreprocessorContext, item: &mut BookItem) -> Result<(), Error> {
            match item {
                BookItem::Chapter(ch) => process_chapter(ctx, ch),
                _ => Ok(()),
            }
        }

        // How we process a regex match
        fn process_match(
            ctx: &PreprocessorContext,
            ch: &Chapter,
            caps: &regex::Captures,
        ) -> String {
            debug!("Processing regex match: {:?}", caps);
            let relative_path = caps.get(1).map(|m| m.as_str()).unwrap();
            debug!("  Relative path: {}", relative_path);

            let page = caps
                .get(2)
                .and_then(|m| m.as_str().parse::<u32>().ok())
                .unwrap();
            debug!("  Page: {}", page);

            let absolute_path = ctx.root.join(relative_path);
            debug!("  Absolute path: {:?}", absolute_path);

            let diagram_name = absolute_path.file_stem().and_then(|s| s.to_str()).unwrap();
            debug!("  Diagram name: {}", diagram_name);

            let svg_name = format!("{}-page-{}.svg", diagram_name, page);
            debug!("  SVG filename: {}", svg_name);

            let result_dir = get_result_dir_abs(&ctx);
            std::fs::create_dir_all(&result_dir).ok();

            let svg_path = &result_dir.join(&svg_name);
            debug!("  SVG path: {:?}", svg_path);

            let svg_relative_path = relative_path_from_chapter(ctx, &ch, &svg_path);
            debug!("  Relative link from chapter: {:?}", svg_relative_path);

            // Export the diagram
            drawio_export(ctx, &absolute_path, page, &svg_path).ok();

            // Create a Markdown snippet for the SVG
            let snippet = format!(
                "![Diagram not found at {}]({})",
                &svg_relative_path.display(),
                &svg_relative_path.display()
            );
            log::debug!("Produced Markdown snippet for SVG: {}", snippet);
            snippet
        }

        // How we process a chapter
        fn process_chapter(ctx: &PreprocessorContext, ch: &mut Chapter) -> Result<(), Error> {
            let re: Regex = directive_regex();
            let cow = re.replace_all(&ch.content, |caps: &regex::Captures| {
                process_match(ctx, &ch, caps)
            });
            ch.content = cow.into_owned();
            for sub in ch.sub_items.iter_mut() {
                process_item(ctx, sub)?;
            }
            Ok(())
        }

        for item in book.sections.iter_mut() {
            process_item(ctx, item)?;
        }
        Ok(book)
    }
}

/// The name of the directory in the book's source that contains the resulting SVG files.
/// Can be set via [preprocessor.drawio.result-dir] in book.toml
pub fn get_result_dir(ctx: &PreprocessorContext) -> &str {
    ctx.config
        .get("preprocessor.drawio.result-dir")
        .and_then(|v| v.as_str())
        .unwrap_or("mdbook-drawio")
}

/// The absolute path to the directory where we store our results
pub fn get_result_dir_abs(ctx: &PreprocessorContext) -> PathBuf {
    let path = &ctx
        .root
        .join(&ctx.config.book.src)
        .join(get_result_dir(ctx));
    path.to_path_buf()
}

/// The name of the drawio binary to use.
/// Can be set via [preprocessor.drawio.drawio-bin] in book.toml
fn get_drawio_bin(ctx: &PreprocessorContext) -> &str {
    ctx.config
        .get("preprocessor.drawio.drawio-bin")
        .and_then(|v| v.as_str())
        .unwrap_or("drawio".into())
}

/// Returns the regular expression used to match drawio directives in markdown files.
/// Intended usage: {{#drawio path="path/to/diagram" page=1}}
pub fn directive_regex() -> Regex {
    Regex::new(r#"\{\{#drawio\s+path=\"([^\"]+)\"\s+page=([0-9]+)[^}]*\}\}"#).unwrap()
}

/// Invokes drawio to export a diagram to SVG format.
fn drawio_export(
    ctx: &PreprocessorContext,
    input: &Path,
    page: u32,
    output_path: &Path,
) -> Result<(), Error> {
    let cli_page = page.to_string();
    let drawio_cmd = get_drawio_bin(ctx);

    debug!("Executing drawio command:");
    debug!("  Command: {}", drawio_cmd);
    debug!("  Input file: {:?}", input);
    debug!("  Output file: {:?}", output_path);
    debug!("  Page: {}", cli_page);

    let mut cmd = Command::new(drawio_cmd);
    cmd.env("ELECTRON_DISABLE_GPU", "1")
        .arg("-x")
        .arg(input)
        .arg("-p")
        .arg(&cli_page)
        .arg("-f")
        .arg("svg")
        .arg("-o")
        .arg(&output_path)
				.arg("--no-sandbox"); // Required for some CI environments

    debug!("Full command: {:?}", cmd);

    let result = cmd.output();

    match result {
        std::result::Result::Ok(output) => {
            debug!("Command exit status: {:?}", output.status);
            debug!(
                "Command stdout: {}",
                String::from_utf8_lossy(&output.stdout)
            );
            if !output.stderr.is_empty() {
                debug!(
                    "Command stderr: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            if !output_path.exists() {
                return Err(Error::msg("Output file was not created"));
            }
        }
        Err(e) => {
            error!("Failed to execute drawio command: {}", e);
            return Err(Error::msg("Output file was not created"));
        }
    }
    Ok(())
}

/// Obtains the relative path from the chapters markdown file to the generated SVG file.
pub fn relative_path_from_chapter(
    ctx: &PreprocessorContext,
    ch: &Chapter,
    target: &Path,
) -> PathBuf {
    // For chapters in src/, we need to create a relative path from the chapter to the SVG
    // Chapter will be at src/chapter.md, target is at gen/drawio/file.svg
    // So we need ../gen/drawio/file.svg from the chapter's perspective
    debug!("Calculating relative path:");
    debug!("  Chapter path: {:?}", ch.path);
    debug!("  Target path: {:?}", target);

    // Extract just the filename from the target path
    let target_filename = target.file_name().and_then(|name| name.to_str()).unwrap();
    debug!("  Target filename: {}", target_filename);

    // Calculate how many directories up we need to go from the chapter's path
    let depth = ch
        .path
        .as_ref()
        .and_then(|p| p.parent())
        .map(|parent| parent.components().count())
        .unwrap_or(0);
    debug!("  Chapter depth: {}", depth);

    // Go as many directories up as the depth indicates
    let up_dirs = if depth == 0 {
        String::from(".")
    } else {
        std::iter::repeat("../").take(depth).collect::<String>()
    };
    let base = Path::new(&up_dirs);
    let result_dir = get_result_dir(ctx);
    let rel_path = base.join(result_dir).join(target_filename);
    debug!("  Relative path from chapter: {:?}", rel_path);
    rel_path
}
