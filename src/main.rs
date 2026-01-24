//! meta-project subprocess plugin

use meta_plugin_protocol::{
    CommandResult, PluginDefinition, PluginHelp, PluginInfo, PluginRequest, run_plugin,
};
use std::collections::HashMap;

fn main() {
    let mut help_commands = HashMap::new();
    help_commands.insert(
        "list".to_string(),
        "List all projects defined in .meta (alias: ls)".to_string(),
    );
    help_commands.insert(
        "check".to_string(),
        "Verify project consistency and health".to_string(),
    );
    help_commands.insert(
        "sync".to_string(),
        "Synchronize project state with .meta config".to_string(),
    );
    help_commands.insert(
        "update".to_string(),
        "Update project dependencies and configs".to_string(),
    );

    run_plugin(PluginDefinition {
        info: PluginInfo {
            name: "project".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            commands: vec![
                "project list".to_string(),
                "project ls".to_string(),
                "project check".to_string(),
                "project sync".to_string(),
                "project update".to_string(),
            ],
            description: Some("Project management for meta repositories".to_string()),
            help: Some(PluginHelp {
                usage: "meta project <command> [args...]".to_string(),
                commands: help_commands,
                examples: vec![
                    "meta project list".to_string(),
                    "meta project list --json".to_string(),
                    "meta project list --recursive".to_string(),
                    "meta project check".to_string(),
                    "meta project sync".to_string(),
                ],
                note: None,
            }),
        },
        execute: execute,
    });
}

fn execute(request: PluginRequest) -> CommandResult {
    if !request.cwd.is_empty() {
        if let Err(e) = std::env::set_current_dir(&request.cwd) {
            return CommandResult::Error(format!("Failed to set working directory: {e}"));
        }
    }

    let options = meta_project_cli::ExecuteOptions {
        dry_run: request.options.dry_run,
        json_output: request.options.json_output,
        recursive: request.options.recursive,
        depth: request.options.depth,
        verbose: request.options.verbose,
    };

    meta_project_cli::execute_command(
        &request.command,
        &request.args,
        &options,
        &request.projects,
    )
}
