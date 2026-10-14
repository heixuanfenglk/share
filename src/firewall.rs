/// Windows 防火墙放行。规则名固定为 `LanShare-{port}`。
/// 若规则是本次创建的，停止时会删掉；启动前已经存在的规则会保留。

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub struct FirewallSession {
    port: u16,
    created: bool,
}

impl FirewallSession {
    pub fn open(port: u16) -> Result<Self, String> {
        #[cfg(windows)]
        {
            if exists(port)? {
                return Ok(Self {
                    port,
                    created: false,
                });
            }
            let args = add_args(port);
            if run_netsh(&args).is_err() && !exists(port)? {
                elevate_netsh(&args)?;
            }
            if exists(port)? {
                Ok(Self {
                    port,
                    created: true,
                })
            } else {
                Err(format!(
                    "防火墙没有放行 TCP {port}。请在授权窗口中允许管理员权限后重试。"
                ))
            }
        }
        #[cfg(not(windows))]
        {
            let _ = port;
            Err("自动放行防火墙目前只支持 Windows。".into())
        }
    }

    pub fn created(&self) -> bool {
        self.created
    }

    pub fn close(&mut self) -> Result<(), String> {
        if !self.created {
            return Ok(());
        }
        self.created = false;
        #[cfg(windows)]
        {
            let args = delete_args(self.port);
            if run_netsh(&args).is_err() && exists(self.port)? {
                elevate_netsh(&args)?;
            }
            if exists(self.port)? {
                Err(format!(
                    "服务已停止，但没能移除防火墙规则 LanShare-{}。",
                    self.port
                ))
            } else {
                Ok(())
            }
        }
        #[cfg(not(windows))]
        {
            Ok(())
        }
    }
}

impl Drop for FirewallSession {
    fn drop(&mut self) {
        if self.created {
            let _ = self.close();
        }
    }
}

#[cfg(windows)]
fn rule_name(port: u16) -> String {
    format!("LanShare-{port}")
}

#[cfg(windows)]
fn add_args(port: u16) -> Vec<String> {
    vec![
        "advfirewall".into(),
        "firewall".into(),
        "add".into(),
        "rule".into(),
        format!("name={}", rule_name(port)),
        "dir=in".into(),
        "action=allow".into(),
        "protocol=TCP".into(),
        format!("localport={port}"),
        "profile=any".into(),
        "enable=yes".into(),
        "description=LanShare".into(),
    ]
}

#[cfg(windows)]
fn delete_args(port: u16) -> Vec<String> {
    vec![
        "advfirewall".into(),
        "firewall".into(),
        "delete".into(),
        "rule".into(),
        format!("name={}", rule_name(port)),
    ]
}

#[cfg(windows)]
fn show_args(port: u16) -> Vec<String> {
    vec![
        "advfirewall".into(),
        "firewall".into(),
        "show".into(),
        "rule".into(),
        format!("name={}", rule_name(port)),
    ]
}

#[cfg(windows)]
pub fn exists(port: u16) -> Result<bool, String> {
    let output = netsh_output(&show_args(port))?;
    Ok(output.status.success())
}

#[cfg(windows)]
fn run_netsh(args: &[String]) -> Result<(), String> {
    let output = netsh_output(args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_text(&output))
    }
}

#[cfg(windows)]
fn netsh_output(args: &[String]) -> Result<std::process::Output, String> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("netsh")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|err| format!("无法运行 netsh：{err}"))
}

#[cfg(windows)]
fn elevate_netsh(args: &[String]) -> Result<(), String> {
    for arg in args {
        if arg
            .chars()
            .any(|ch| matches!(ch, '&' | '|' | '>' | '<' | '"' | '%' | '\n' | '\r' | '\''))
        {
            return Err("防火墙参数包含不支持的字符。".into());
        }
    }
    let argument_list = args
        .iter()
        .map(|arg| format!("'{arg}'"))
        .collect::<Vec<_>>()
        .join(",");
    let script = format!(
        "Start-Process -FilePath 'netsh' -ArgumentList {argument_list} -Verb RunAs -Wait -WindowStyle Hidden"
    );
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|err| format!("无法请求管理员权限：{err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let detail = command_text(&output);
        if detail.is_empty() {
            Err("没有获得管理员权限，防火墙端口未放行。".into())
        } else {
            Err(format!("没有获得管理员权限，防火墙端口未放行。{detail}"))
        }
    }
}

#[cfg(windows)]
fn command_text(output: &std::process::Output) -> String {
    let stdout = decode_console(&output.stdout);
    let stderr = decode_console(&output.stderr);
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout,
        (true, false) => stderr,
        (false, false) => format!("{stdout} {stderr}"),
    }
}

#[cfg(windows)]
fn decode_console(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        let trimmed = text.trim();
        if !trimmed.is_empty() && !trimmed.contains('\u{fffd}') {
            return trimmed.to_string();
        }
    }
    let (text, _, _) = encoding_rs::GBK.decode(bytes);
    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn missing_rule_is_not_present() {
        #[cfg(windows)]
        {
            let present = super::exists(59999).expect("netsh show rule");
            assert!(!present);
        }
    }
}
