//! Printing, the flows both clients run: looking for printers, and sending
//! a document from this machine or from the device's file store. The words
//! are in `describe::printer`.

use std::path::Path;

use protocol::api::{self, PrintQuery};
use protocol::{PrintQueued, PrinterFound, PRINT_DATA_MAX};

use crate::connect::Session;

/// Every printer the device finds on USB and the network. `each` sees them
/// as they come; `stop` ends the wait early.
pub fn discover(
    session: &mut Session,
    stop: &dyn Fn() -> bool,
    mut each: impl FnMut(&PrinterFound),
) -> Result<Vec<PrinterFound>, String> {
    let mut found = Vec::new();
    let mut failed = None;
    session.job::<api::printer::Discover, PrinterFound>((), stop, |event| match event {
        Ok(printer) => {
            each(&printer);
            found.push(printer);
        }
        Err(event) => failed = Some(format!("the device sent {event}")),
    })?;
    match failed {
        Some(err) => Err(err),
        None => Ok(found),
    }
}

/// How a document is printed, besides what it is.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub copies: Option<u32>,
    pub media: Option<String>,
    pub title: Option<String>,
}

/// Print a file from this machine: sent with the request, so at most
/// `PRINT_DATA_MAX` bytes. Its name is the job's title unless one is given.
pub fn print_file(
    session: &mut Session,
    printer: &str,
    file: &Path,
    options: Options,
) -> Result<PrintQueued, String> {
    let bytes = std::fs::read(file).map_err(|err| format!("{}: {err}", file.display()))?;
    if bytes.is_empty() {
        return Err(format!("{}: the file is empty", file.display()));
    }
    if bytes.len() > PRINT_DATA_MAX {
        return Err(format!(
            "{}: a document sent to print is at most {}; upload it with \
             `tessaro-ctl files upload` and print it with --stored",
            file.display(),
            protocol::size_label(PRINT_DATA_MAX as u64)
        ));
    }
    let title = options.title.or_else(|| {
        file.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    });
    session
        .upload::<api::printer::Print>(
            PrintQuery {
                printer: printer.to_string(),
                path: None,
                copies: options.copies,
                media: options.media,
                title,
            },
            &bytes,
        )
        .into_result()
}

/// Print a file from the device's store, by its path there.
pub fn print_stored(
    session: &mut Session,
    printer: &str,
    path: &str,
    options: Options,
) -> Result<PrintQueued, String> {
    let title = options.title.or_else(|| {
        path.rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    });
    session
        .upload::<api::printer::Print>(
            PrintQuery {
                printer: printer.to_string(),
                path: Some(path.to_string()),
                copies: options.copies,
                media: options.media,
                title,
            },
            &[],
        )
        .into_result()
}
