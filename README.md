# IsaacLab 训练管理器

一个现代化的Web界面，用于管理和监控 IsaacLab 强化学习训练任务。

## ✨ 主要功能

- **网页化任务管理**: 通过简单直观的Web界面创建、监控和管理您的所有IsaacLab训练任务。
- **实时日志和指标**: 直接在浏览器中查看实时任务日志和训练指标图表。
- **代码同步**: 高效地将本地代码同步到服务器进行训练。
- **集中化配置**: 所有系统配置均可通过UI中的“系统配置”页面进行管理，无需手动编辑配置文件。

## 🚀 快速启动

### 1. 先决条件

- [Rust](https://www.rust-lang.org/tools/install) (最新稳定版)
- [Conda](https://docs.conda.io/projects/conda/en/latest/user-guide/install/index.html)
- 一个配置好并可以运行 IsaacLab 的 Conda 环境。

### 2. 安装与运行

1.  **克隆项目**
    ```bash
    git clone <your-repo-url>
    cd isaaclab-manager
    ```

2.  **构建并运行**
    ```bash
    cargo run --release
    ```
    应用启动时会自动运行数据库迁移，并使用默认值填充配置（如果数据库为空）。

3.  **访问Web界面**
    打开浏览器并访问 `http://localhost:6006` (或您在配置页面中设置的任何主机和端口)。

## ⚙️ 配置

所有系统配置项，例如 IsaacLab 路径、Conda 环境、服务器端口等，现在都通过Web界面中的 **系统配置** 标签页进行管理。

**注意**: `config/app.toml` 配置文件已被弃用，不再使用。首次启动后，所有配置将从数据库加载和保存。
