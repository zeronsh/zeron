# SSH 直连模式：手机直接连你的 Windows 电脑

不需要 Zeron 云账号，也不经过中继。手机通过 SSH 登录你的电脑，再经 SSH 隧道
（direct-tcpip）连到本机的 Zeron 引擎（`127.0.0.1:27654`）。引擎端口不对外开放，
只开放 SSH（22）。

```
手机 ──SSH(22)──▶ Windows sshd ──隧道──▶ 127.0.0.1:27654 (Zeron 引擎)
```

以下命令都在**管理员 PowerShell** 中执行（开始菜单右键 → 终端(管理员)）。

## 1. 安装并启动 OpenSSH Server

```powershell
Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0
Start-Service sshd
Set-Service sshd -StartupType Automatic
```

检查防火墙规则（安装时通常会自动创建）：

```powershell
Get-NetFirewallRule -Name *OpenSSH-Server* | Select-Object Name, Enabled, Profile
```

如果没有输出，手动添加：

```powershell
New-NetFirewallRule -Name OpenSSH-Server-In-TCP -DisplayName "OpenSSH Server (sshd)" -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22
```

## 2. 添加手机的公钥

在手机上点首页右上角的头像图标进入 **设置 → 账户与电脑**，在页面底部 **本机 SSH 密钥** 下点 **复制公钥**，发到电脑上
（微信文件传输助手、邮件等都可以）。整行形如 `ssh-ed25519 AAAA... zeron-xxx`。

管理员账户的公钥必须放在 `administrators_authorized_keys`（不是用户目录下的
`.ssh\authorized_keys`）：

```powershell
$key = 'ssh-ed25519 AAAA...把手机上复制的整行粘贴到这里...'
Add-Content -Path C:\ProgramData\ssh\administrators_authorized_keys -Value $key -Encoding ascii
icacls C:\ProgramData\ssh\administrators_authorized_keys /inheritance:r /grant "Administrators:F" /grant "SYSTEM:F"
```

> 权限不对（例如继承了 Users 的读取权限）时，sshd 会静默忽略这个文件，手机会提示认证失败。

如果你的 Windows 账户**不是**管理员，改为：

```powershell
New-Item -ItemType Directory -Force $env:USERPROFILE\.ssh | Out-Null
Add-Content -Path $env:USERPROFILE\.ssh\authorized_keys -Value $key -Encoding ascii
```

也可以不用公钥，在手机上选择"密码"登录（使用 Windows 账户密码；微软账户登录的电脑用微软账户密码）。

## 3. 查电脑的局域网 IP

```powershell
ipconfig
```

找到正在使用的网卡（WLAN 或 以太网）下的 **IPv4 地址**，形如 `192.168.x.x`。
手机和电脑需在同一局域网（或通过 Tailscale/ZeroTier 等组网，填对应 IP）。

## 4. 让 Zeron 引擎保持运行

最简单：电脑上保持 Zeron 应用打开。

或者登录时自动以无界面模式启动引擎。Zeron 是免安装的单个 exe，先确定它的路径
（Zeron 正在运行时可以直接查）：

```powershell
(Get-Process zeron).Path
```

把输出的路径填进 `$exe`，然后创建登录任务：

```powershell
$exe = "C:\Tools\Zeron\zeron.exe"
schtasks /Create /TN "Zeron Headless" /SC ONLOGON /RL LIMITED /TR "\"$exe\" headless"
schtasks /Run /TN "Zeron Headless"
```

## 5. 验证

引擎在监听 27654：

```powershell
netstat -ano | findstr 27654
```

应看到 `127.0.0.1:27654 ... LISTENING`。

sshd 在监听 22：

```powershell
netstat -ano | findstr ":22 "
```

## 6. 核对主机指纹（首次连接时）

```powershell
ssh-keygen -lf C:\ProgramData\ssh\ssh_host_ed25519_key.pub
```

输出形如 `256 SHA256:xxxxxxxx... (ED25519)`。手机第一次连接时会弹出
**信任这台电脑吗？**，显示 `SHA256:...` 指纹，**两者一致再点 信任**。
之后如果指纹变化，手机会拒绝连接并提示 **主机密钥已变更**（可能是重装系统，
也可能是中间人攻击）。

## 7. 在手机上添加机器

**设置 → 账户与电脑 → 添加电脑**（或 **设置 → 添加电脑（SSH）…**）：

| 字段 | 填写 |
| --- | --- |
| 名称 | 随意，例如 `家里的电脑` |
| 主机 | 第 3 步的 IPv4 地址 |
| SSH 端口 | `22` |
| Zeron 端口 | `27654`（默认） |
| 用户名 | Windows 用户名（`$env:USERNAME` 的输出） |
| 登录方式 | 本机密钥（推荐）/ 导入密钥 / 密码 |

点 **测试** → 核对指纹 → **信任** → 看到 **已连接 · Zeron 0.2.x 响应用时 … ms** → **保存并连接**。

## 常见问题

- **Connect 失败 / 超时**：确认 IP、同一网络、防火墙 22 端口放行；`Get-Service sshd` 状态为 Running。
- **Auth 失败**：公钥文件路径和 `icacls` 权限；用户名是否正确。可在电脑上看日志：
  `Get-WinEvent -LogName OpenSSH/Operational -MaxEvents 20 | Format-List TimeCreated, Message`
- **SSH 成功但引擎连不上**：Zeron 没在运行（第 4、5 步）。
- **Windows 更新后 sshd 服务不见了**（`Get-Service sshd` 找不到服务）：用 `New-Service` 重新注册，命令见 [README.zh-CN.md 的常见问题](../README.zh-CN.md#常见问题)。
- **sshd 禁用了端口转发**：检查 `C:\ProgramData\ssh\sshd_config` 中没有 `AllowTcpForwarding no`；修改后 `Restart-Service sshd`。
- **连上了但会话列表是空的 / 一直在加载**：会话页顶部会显示连接状态横幅（连接中 / 正在加载会话 / 错误原因）。
  点 **详情**（或 **设置 → 连接详情**）查看引擎版本、每个数据流（WatchDevices / WatchSpaces / WatchChats / WatchSessions）收到的帧数和行数，以及连接日志；
  点右上角 **复制** 可把这份诊断文本（不含密钥）复制出来反馈。20 秒内有数据流一直没有数据时，会提示是哪一个并自动重连。
- **电脑上的 Zeron 更新了，手机 App 要不要跟着更新？** 一般不用。App 对引擎版本变化做了容错：
  多出来的字段直接忽略；不认识的状态、模型档位、harness 等取值按默认值显示，并在详情里记为"N 行忽略了未知值"；
  对话里不认识的消息类型或工具类型显示成一条 "Unsupported content" 占位，其余内容照常显示；引擎新增的通知类消息直接忽略；
  实在读不了的行才跳过，并在横幅和详情里提示跳过了几行。
  补丁版本（例如 0.2.97 → 0.2.98）不提示；引擎的主/次版本比 App 测试过的新（例如 0.3.x）时，会话页顶部出现一张可关闭的提示卡，
  建议有空时更新 App，但不影响使用。

## 目前的限制

- 消息可以带图片/文件附件，但要求电脑上的 Zeron ≥ 0.2.12（引擎太旧时 app 会提示先升级电脑端）。
- 共享消息队列取决于引擎能力：支持的引擎上，会话忙时发送的消息进入共享队列；不支持的旧版本上会作为"插话（steer）"发给当前轮次。
- 置顶和分组在手机上的编辑只保存在手机本地；电脑自己的置顶会镜像显示到手机（不写回电脑）。
- 只显示引擎所在电脑的在线状态。
