// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

pub async fn load(ctx: &mut super::Context) -> Result<(), super::LspError> {
    use i_slint_compiler::diagnostics::BuildDiagnostics;
    use std::collections::{HashMap, HashSet};

    let roots: Vec<_> = crate::host_language_search::resolve_workspace_folders(&ctx.init_param)
        .into_iter()
        .filter_map(|folder| folder.uri.to_file_path().ok())
        .collect();
    if roots.is_empty() {
        return Ok(());
    }
    let dc = &mut ctx.session.document_cache;
    let cached: HashMap<_, _> = dc
        .all_url_documents()
        .filter(|(url, _)| url.scheme() == "file")
        .map(|(url, node)| (url, node.source_file.clone()))
        .collect();
    let cached_urls: Vec<_> = cached.keys().cloned().collect();
    let open: HashSet<_> = ctx
        .session
        .open_urls
        .iter()
        .filter_map(|url| i_slint_compiler::source_path::SourcePath::from_url(url).to_url())
        .collect();
    let known_paths: Vec<_> = cached
        .keys()
        .filter(|url| url.path().ends_with(".slint"))
        .filter_map(|url| url.to_file_path().ok())
        .collect();
    let sources = tokio::task::spawn_blocking(move || {
        let mut files: HashSet<_> = known_paths.into_iter().collect();
        let mut directories = roots;
        let mut visited = HashSet::new();
        while let Some(directory) = directories.pop() {
            if !visited.insert(directory.clone()) {
                continue;
            }
            let entries = match std::fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(format!("Cannot scan {}: {error}", directory.display()));
                }
            };
            for entry in entries {
                let entry = entry
                    .map_err(|error| format!("Cannot scan {}: {error}", directory.display()))?;
                let kind = match entry.file_type() {
                    Ok(kind) => kind,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(format!("Cannot inspect {}: {error}", entry.path().display()));
                    }
                };
                let path = entry.path();
                if kind.is_dir() {
                    if !matches!(
                        entry.file_name().to_str(),
                        Some(".git" | "target" | "node_modules")
                    ) {
                        directories.push(path);
                    }
                } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "slint") {
                    files.insert(path);
                }
            }
        }
        let mut sources = Vec::new();
        for path in files {
            let url = lsp_types::Url::from_file_path(&path)
                .map_err(|()| format!("Cannot represent {} as a URL", path.display()))?;
            if open.contains(&url) {
                continue;
            }
            let source = match std::fs::read_to_string(&path) {
                Ok(source) => Some(source),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(format!("Cannot read {}: {error}", path.display())),
            };
            let previous = cached.get(&url).map(|file| file.source().unwrap_or_default());
            if source.as_deref() != previous {
                sources.push((url, source));
            }
        }
        Ok(sources)
    })
    .await;
    let sources = sources.map_err(|error| error.to_string()).flatten().map_err(|message| {
        super::LspError {
            code: super::LspErrorCode::RequestFailed,
            message: format!("Workspace reference scan failed: {message}"),
        }
    })?;
    for (url, _) in &sources {
        if let Err(error) = dc.drop_document(url) {
            tracing::debug!("Cannot invalidate {url}: {error}");
        }
    }
    for (url, source) in sources {
        if let Some(source) = source
            && dc.get_document(&url).is_none()
        {
            dc.reload_cached_file(&url, &mut BuildDiagnostics::default()).await;
            if dc.get_document(&url).is_none() {
                let _ = dc.load_url(&url, None, source, &mut BuildDiagnostics::default()).await;
            }
        }
    }
    for url in cached_urls {
        dc.reload_cached_file(&url, &mut BuildDiagnostics::default()).await;
    }
    Ok(())
}
