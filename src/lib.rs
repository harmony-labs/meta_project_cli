use meta_plugin_api::{Plugin, PluginError};

pub struct ProjectPlugin;

impl Plugin for ProjectPlugin {
    fn name(&self) -> &'static str {
        "project"
    }

    fn commands(&self) -> Vec<&'static str> {
        vec!["project sync", "project update"]
    }

    fn execute(&self, command: &str, args: &[String]) -> anyhow::Result<()> {
        match command {
            "project sync" => {
                println!("Syncing projects (placeholder)");
                // Future: implement interactive sync wizard
                Ok(())
            }
            "project update" => {
                println!("Updating projects (placeholder)");
                // Future: clone missing repos, update existing
                Ok(())
            }
            _ => Err(PluginError::CommandNotFound(command.to_string()).into()),
        }
    }
}

#[no_mangle]
pub extern "C" fn _plugin_create() -> *mut dyn Plugin {
    Box::into_raw(Box::new(ProjectPlugin))
}