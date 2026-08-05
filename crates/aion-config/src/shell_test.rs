use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detect_shell_kind_recognizes_supported_shells() {
        assert_eq!(detect_shell_kind("bash"), Some(ShellKind::Bash));
        assert_eq!(detect_shell_kind("bash.exe"), Some(ShellKind::Bash));
        assert_eq!(detect_shell_kind("/bin/zsh"), Some(ShellKind::Zsh));
        assert_eq!(detect_shell_kind("/bin/sh"), Some(ShellKind::Sh));
        assert_eq!(detect_shell_kind("pwsh"), Some(ShellKind::PowerShell));
        assert_eq!(detect_shell_kind("powershell.exe"), Some(ShellKind::PowerShell));
        assert_eq!(
            detect_shell_kind(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"),
            Some(ShellKind::PowerShell)
        );
        assert_eq!(detect_shell_kind("cmd.exe"), Some(ShellKind::Cmd));
        assert_eq!(detect_shell_kind("fish"), None);
        assert_eq!(detect_shell_kind("nu"), None);
    }

    #[test]
    fn detect_shell_kind_recognizes_unicode_paths() {
        assert_eq!(detect_shell_kind(r"C:\用户\工具\pwsh.exe"), Some(ShellKind::PowerShell));
    }

    #[test]
    fn derive_exec_args_uses_shell_specific_flags() {
        let bash = ResolvedShell::new(ShellKind::Bash, PathBuf::from("/bin/bash"));
        assert_eq!(bash.derive_exec_args("echo ok", false), vec!["-c", "echo ok"]);
        assert_eq!(bash.derive_exec_args("echo ok", true), vec!["-lc", "echo ok"]);

        let powershell = ResolvedShell::new(ShellKind::PowerShell, PathBuf::from("pwsh"));
        assert_eq!(
            powershell.derive_exec_args("Write-Output ok", false),
            vec![
                "-NoProfile",
                "-Command",
                "try { $utf8=New-Object System.Text.UTF8Encoding $false; [Console]::OutputEncoding=$utf8; $OutputEncoding=$utf8 } catch {}\nWrite-Output ok"
            ]
        );

        let cmd = ResolvedShell::new(ShellKind::Cmd, PathBuf::from("cmd.exe"));
        assert_eq!(cmd.derive_exec_args("echo ok", false), vec!["/C", "echo ok"]);
    }

    #[tokio::test]
    async fn shell_command_runs_echo() {
        let output = shell_command("echo hello").await.expect("shell_command failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("hello"));
    }

    #[tokio::test]
    async fn shell_command_builder_allows_env_and_cwd() {
        let tmp = std::env::temp_dir();
        let shell = default_shell();
        let cmd_str = match shell.kind {
            ShellKind::PowerShell => "Write-Output $env:MY_VAR",
            ShellKind::Cmd => "echo %MY_VAR%",
            ShellKind::Bash | ShellKind::Zsh | ShellKind::Sh => "echo $MY_VAR",
        };
        let output = shell_command_builder(&shell, cmd_str, false)
            .env("MY_VAR", "test_value")
            .current_dir(&tmp)
            .output()
            .await
            .expect("builder failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("test_value"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn powershell_native_pipeline_uses_utf8() {
        let shell = default_shell();
        let command = r#"'定时任务' | & ([Diagnostics.Process]::GetCurrentProcess().MainModule.FileName) -NoProfile -Command '$stream=[Console]::OpenStandardInput(); $memory=New-Object IO.MemoryStream; $stream.CopyTo($memory); [BitConverter]::ToString($memory.ToArray()).Replace(''-'','''')'"#;

        let output = shell_command_builder(&shell, command, false)
            .output()
            .await
            .expect("PowerShell pipeline failed");

        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "E5AE9AE697B6E4BBBBE58AA10D0A"
        );
    }
}
