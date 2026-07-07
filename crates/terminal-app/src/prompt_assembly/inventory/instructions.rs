use super::*;

pub(in crate::prompt_assembly) fn discover_instruction_files(
    work_dir: &Path,
    global_instructions_path_override: Option<&Path>,
) -> (
    Vec<DiscoveredInstructionsFile>,
    Vec<PromptAssemblyDiagnostic>,
) {
    let mut discovered = Vec::new();
    let mut diagnostics = Vec::new();

    if let Some(global_file) = global_instructions_path_override
        .map(Path::to_path_buf)
        .or_else(global_instructions_file_path)
        .filter(|path| path.is_file())
    {
        match load_instructions_file(
            "instructions:global",
            "Global AGENTS.md".to_string(),
            &global_file,
            PromptSourceOrigin::Global,
        ) {
            Ok(Some(file)) => discovered.push(file),
            Ok(None) => {}
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }

    let project_root = git_root(work_dir);
    let search_dirs = match project_root.as_deref() {
        Some(root) => project_instruction_search_dirs(root, work_dir),
        None => vec![work_dir.to_path_buf()],
    };

    for directory in search_dirs {
        let Some(path) = first_instruction_file_in_dir(&directory) else {
            continue;
        };
        let reference_id = project_instruction_reference_id(project_root.as_deref(), &directory);
        let title = project_instruction_title(project_root.as_deref(), &path);
        match load_instructions_file(&reference_id, title, &path, PromptSourceOrigin::Project) {
            Ok(Some(file)) => discovered.push(file),
            Ok(None) => {}
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }

    (discovered, diagnostics)
}

pub(in crate::prompt_assembly) fn load_instructions_file(
    reference_id: &str,
    title: String,
    path: &Path,
    origin: PromptSourceOrigin,
) -> Result<Option<DiscoveredInstructionsFile>, PromptAssemblyDiagnostic> {
    let body = fs::read_to_string(path).map_err(|error| PromptAssemblyDiagnostic {
        origin: Some(origin),
        path: Some(path.to_path_buf()),
        message: format!("read instructions file: {error}"),
    })?;
    let body = body.trim().to_string();
    if body.is_empty() {
        return Ok(None);
    }

    Ok(Some(DiscoveredInstructionsFile {
        reference_id: reference_id.to_string(),
        title,
        path: path.to_path_buf(),
        body,
        origin,
    }))
}

pub(in crate::prompt_assembly) fn global_instructions_file_path() -> Option<PathBuf> {
    hunea_config_dir().map(|dir| dir.join(GLOBAL_INSTRUCTIONS_FILE_NAME))
}

pub(in crate::prompt_assembly) fn project_instruction_search_dirs(
    project_root: &Path,
    work_dir: &Path,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut cursor = Some(work_dir);
    while let Some(dir) = cursor {
        dirs.push(dir.to_path_buf());
        if dir == project_root {
            break;
        }
        cursor = dir.parent();
    }
    dirs.reverse();
    dirs
}

pub(in crate::prompt_assembly) fn first_instruction_file_in_dir(dir: &Path) -> Option<PathBuf> {
    PROJECT_INSTRUCTIONS_FILE_NAMES
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

pub(in crate::prompt_assembly) fn project_instruction_reference_id(
    project_root: Option<&Path>,
    directory: &Path,
) -> String {
    let relative = project_root
        .and_then(|root| directory.strip_prefix(root).ok())
        .map(path_component_key)
        .unwrap_or_else(|| ".".to_string());
    format!("instructions:project:{relative}")
}

pub(in crate::prompt_assembly) fn project_instruction_title(
    project_root: Option<&Path>,
    path: &Path,
) -> String {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(GLOBAL_INSTRUCTIONS_FILE_NAME);
    let Some(parent) = path.parent() else {
        return file_name.to_string();
    };
    let relative_directory = project_root
        .and_then(|root| parent.strip_prefix(root).ok())
        .map(path_component_key)
        .unwrap_or_else(|| ".".to_string());
    if relative_directory == "." {
        file_name.to_string()
    } else {
        format!("{relative_directory}/{file_name}")
    }
}

pub(in crate::prompt_assembly) fn ensure_discovered_instruction_entries(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    discovered_instruction_files: &[DiscoveredInstructionsFile],
) {
    let mut global_files = discovered_instruction_files
        .iter()
        .filter(|file| file.origin == PromptSourceOrigin::Global)
        .cloned()
        .collect::<Vec<_>>();
    let mut project_files = discovered_instruction_files
        .iter()
        .filter(|file| file.origin == PromptSourceOrigin::Project)
        .cloned()
        .collect::<Vec<_>>();
    global_files.sort_by(|left, right| left.reference_id.cmp(&right.reference_id));
    project_files.sort_by(|left, right| left.reference_id.cmp(&right.reference_id));

    ensure_scope_instruction_entries(
        global_state,
        &global_files,
        DEFAULT_INSTRUCTIONS_REQUESTED_ORDER_START,
    );
    ensure_scope_instruction_entries(
        project_state,
        &project_files,
        DEFAULT_INSTRUCTIONS_REQUESTED_ORDER_START
            .saturating_add(u16::try_from(global_files.len()).unwrap_or(u16::MAX)),
    );
}

pub(in crate::prompt_assembly) fn ensure_scope_instruction_entries(
    state: &mut PromptAssemblyScopeState,
    discovered_instruction_files: &[DiscoveredInstructionsFile],
    starting_order: u16,
) {
    let mut next_instruction_order = state
        .entries()
        .iter()
        .filter(|entry| entry.kind == PromptSourceKind::InstructionsFile)
        .filter_map(|entry| entry.requested_order)
        .max()
        .map(|order| order.saturating_add(1))
        .unwrap_or(starting_order);

    for file in discovered_instruction_files {
        if let Some(entry) = state.entry_mut(PromptSourceKind::InstructionsFile, &file.reference_id)
        {
            entry.title = file.title.clone();
            continue;
        }

        state.upsert_entry(PersistedPromptAssemblyEntry {
            reference_id: file.reference_id.clone(),
            kind: PromptSourceKind::InstructionsFile,
            title: file.title.clone(),
            enabled: true,
            requested_order: Some(next_instruction_order),
        });
        next_instruction_order = next_instruction_order.saturating_add(1);
    }
}
