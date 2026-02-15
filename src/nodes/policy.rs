//! Platform-specific command policies for node invocations

use std::collections::HashSet;

/// Platform-specific default allowed commands
#[must_use]
pub fn platform_defaults(platform: &str) -> HashSet<String> {
    let mut cmds = HashSet::new();

    // Common across all platforms
    for cmd in [
        "device.info",
        "device.status",
        "canvas.present",
        "canvas.hide",
        "canvas.navigate",
        "canvas.eval",
        "canvas.snapshot",
    ] {
        cmds.insert(cmd.to_string());
    }

    match platform {
        "darwin" | "linux" | "windows" => {
            for cmd in [
                "system.run",
                "system.which",
                "system.notify",
                "browser.proxy",
            ] {
                cmds.insert(cmd.to_string());
            }
        }
        "ios" | "android" => {
            for cmd in [
                "camera.list",
                "camera.snap",
                "location.get",
                "contacts.search",
                "calendar.events",
                "photos.latest",
            ] {
                cmds.insert(cmd.to_string());
            }
        }
        _ => {}
    }

    cmds
}

/// Check if a command is allowed for a given platform + node declaration
#[must_use]
pub fn is_command_allowed(
    platform: &str,
    declared_commands: &[String],
    deny_list: &HashSet<String>,
    command: &str,
) -> bool {
    let defaults = platform_defaults(platform);

    // Must be in platform defaults AND declared by the node
    defaults.contains(command)
        && declared_commands.iter().any(|c| c == command)
        && !deny_list.contains(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_has_system_commands() {
        let cmds = platform_defaults("darwin");
        assert!(cmds.contains("system.run"));
        assert!(cmds.contains("browser.proxy"));
        assert!(!cmds.contains("camera.snap"));
    }

    #[test]
    fn mobile_has_device_commands() {
        let cmds = platform_defaults("ios");
        assert!(cmds.contains("camera.snap"));
        assert!(cmds.contains("location.get"));
        assert!(!cmds.contains("system.run"));
    }

    #[test]
    fn common_commands_on_all_platforms() {
        for platform in ["darwin", "linux", "windows", "ios", "android"] {
            let cmds = platform_defaults(platform);
            assert!(cmds.contains("device.info"), "missing device.info on {platform}");
            assert!(cmds.contains("canvas.present"), "missing canvas.present on {platform}");
        }
    }

    #[test]
    fn allowed_requires_platform_and_declaration() {
        let deny = HashSet::new();
        let declared = vec!["device.info".to_string(), "system.run".to_string()];

        assert!(is_command_allowed("darwin", &declared, &deny, "device.info"));
        assert!(is_command_allowed("darwin", &declared, &deny, "system.run"));
        // Not declared
        assert!(!is_command_allowed("darwin", &declared, &deny, "browser.proxy"));
    }

    #[test]
    fn deny_list_blocks_command() {
        let mut deny = HashSet::new();
        deny.insert("system.run".to_string());
        let declared = vec!["system.run".to_string()];

        assert!(!is_command_allowed("darwin", &declared, &deny, "system.run"));
    }
}
