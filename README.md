# IsaacLab训练管理服务 - 详细安装指南

本指南将帮助您完整安装和配置IsaacLab训练管理服务，支持conda环境管理。

## 📋 系统要求检查

### 1. 操作系统

* Ubuntu 20.04 LTS 或更高版本
* CentOS 8 或更高版本
* 其他Linux发行版

### 2. 硬件要求

* **CPU** : 4核心以上推荐
* **内存** : 8GB以上推荐
* **GPU** : NVIDIA GPU (支持CUDA)，用于IsaacLab训练
* **存储** : 50GB以上可用空间

## 🛠 环境准备

### 步骤1: 安装系统依赖

```bash
# Ubuntu/Debian
sudo apt update
sudo apt install -y curl wget git build-essential

# CentOS/RHEL
sudo yum update
sudo yum groupinstall -y "Development Tools"
sudo yum install -y curl wget git
```

### 步骤2: 安装Rust

```bash
# 安装Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# 验证安装
rustc --version
cargo --version
```

### 步骤3: 安装Conda

```bash
# 下载Miniconda
wget https://repo.anaconda.com/miniconda/Miniconda3-latest-Linux-x86_64.sh

# 安装
bash Miniconda3-latest-Linux-x86_64.sh

# 重新加载环境
source ~/.bashrc

# 验证安装
conda --version
```

### 步骤4: 安装rsync

```bash
# Ubuntu/Debian
sudo apt install -y rsync

# CentOS/RHEL
sudo yum install -y rsync

# 验证安装
rsync --version
```

## 🤖 IsaacLab安装

### 方法1: 使用Isaac Sim安装

1. **下载Isaac Sim**
   * 访问 [NVIDIA Isaac Sim官网](https://developer.nvidia.com/isaac-sim)
   * 下载最新版本的Isaac Sim
2. **安装Isaac Sim**
   ```bash
   # 按照官方指导安装Isaac Sim
   # 通常安装在 ~/isaac-sim 或 /opt/isaac-sim
   ```
3. **安装IsaacLab**
   ```bash
   # 克隆IsaacLab仓库
   git clone https://github.com/isaac-sim/IsaacLab.git /opt/isaaclab
   cd /opt/isaaclab

   # 创建符号链接到Isaac Sim
   ln -s ~/isaac-sim _isaac_sim
   ```

### 方法2: 使用conda安装(如果可用)

```bash
# 如果有conda包可用
conda install -c conda-forge isaaclab
```

## 🐍 Conda环境配置

### 使用自动配置脚本

```bash
# 下载并运行conda配置脚本
wget https://your-server.com/conda_setup.sh
chmod +x conda_setup.sh
./conda_setup.sh
```

### 手动配置

```bash
# 创建IsaacLab环境
conda create -n isaaclab python=3.8 -y
conda activate isaaclab

# 安装PyTorch (CUDA版本)
conda install pytorch torchvision torchaudio pytorch-cuda=11.8 -c pytorch -c nvidia -y

# 安装科学计算库
pip install numpy scipy matplotlib pandas

# 安装机器学习库
pip install tensorboard wandb stable-baselines3

# 安装IsaacLab依赖
cd /opt/isaaclab
pip install -e .

# 测试安装
python -c "import omni.isaac.lab; print('IsaacLab安装成功!')"
```

## 🚀 安装训练管理服务

### 步骤1: 克隆项目

```bash
# 克隆项目
git clone https://github.com/your-username/isaaclab-manager.git
cd isaaclab-manager
```

### 步骤2: 一键安装

```bash
# 给安装脚本执行权限
chmod +x setup.sh

# 运行安装脚本
./setup.sh
```

* 构建Rust应用
* 初始化数据库
* 创建systemd服务
* 启动服务

### 步骤3: 验证安装

```bash
# 检查服务状态
sudo systemctl status isaaclab-manager

# 检查日志
sudo journalctl -u isaaclab-manager -f

# 测试Web界面
curl http://localhost:3000
```

## ⚙️ 配置详解

### 主配置文件: config/app.toml

```toml
[server]
host = "0.0.0.0"          # 服务器监听地址
port = 3000               # 服务器端口

[isaaclab]
path = "/opt/isaaclab"                    # IsaacLab安装路径
python_executable = "python"             # Python可执行文件
conda_path = "/opt/miniconda3"           # Conda安装路径
default_conda_env = "isaaclab"           # 默认conda环境

[storage]
output_path = "./outputs"                 # 训练输出目录
log_path = "./logs"                      # 日志目录
database_url = "sqlite:./data/isaaclab_manager.db"  # 数据库文件

[tensorboard]
base_port = 6006         # TensorBoard基础端口
max_instances = 10       # 最大TensorBoard实例数

[sync]
target_path = "/opt/isaaclab/source"     # 代码同步目标路径
default_excludes = [                     # 默认排除模式
    "__pycache__",
    "*.pyc",
    ".git",
    "logs/",
    "outputs/"
]

[tasks]
max_concurrent = 1       # 最大并发任务数
default_headless = true  # 默认无头模式
timeout_seconds = 86400  # 任务超时时间(秒)
```

### 环境变量配置

创建 `.env` 文件：

```bash
# 服务器配置
SERVER_HOST=0.0.0.0
SERVER_PORT=3000

# IsaacLab配置
ISAACLAB_PATH=/opt/isaaclab
CONDA_PATH=/opt/miniconda3
CONDA_ENV=isaaclab

# 数据库配置
DATABASE_URL=sqlite:./data/isaaclab_manager.db

# 日志级别
RUST_LOG=info
```

## 🎮 使用指南

### 1. 访问Web界面

打开浏览器，访问: http://localhost:3000

### 2. 创建训练任务

1. **选择任务类型**
   * G1-Walk-v0: G1机器人行走训练
   * G1-Run-v0: G1机器人跑步训练
   * Cartpole-v1: 倒立摆控制
   * Ant-v2: 蚂蚁机器人
   * Humanoid-v2: 人形机器人
2. **选择conda环境**
   * 从下拉列表选择已配置的环境
   * 确保环境中已安装IsaacLab
3. **配置参数**
   * 无头模式: 不显示图形界面
   * 恢复训练: 从检查点继续
   * 自定义参数: 添加额外命令行参数
4. **创建任务**
   * 点击"创建任务"按钮
   * 任务将自动加入队列

### 3. 监控任务

* **查看队列** : 在"任务管理"页面查看排队任务
* **实时日志** : 点击"日志"按钮查看实时输出
* **TensorBoard** : 点击"TensorBoard"查看训练曲线
* **任务状态** : 监控任务执行状态

### 4. 代码同步

1. 进入"代码同步"选项卡
2. 设置本地代码路径
3. 配置排除文件模式
4. 点击"开始同步"

### 5. 下载模型

* 训练完成后，点击"下载ONNX"
* 自动下载生成的模型文件

## 🔧 常见问题排查

### 问题1: 服务启动失败

 **现象** : systemctl启动失败

 **排查步骤** :

```bash
# 查看详细错误
sudo journalctl -u isaaclab-manager --no-pager -l

# 检查配置文件
cat config/app.toml

# 手动启动测试
./target/release/isaaclab-manager
```

 **常见原因** :

* IsaacLab路径不正确
* Conda路径不正确
* 端口被占用
* 权限不足

### 问题2: conda环境加载失败

 **现象** : 任务创建后无法启动

 **排查步骤** :

```bash
# 检查conda安装
conda --version

# 列出可用环境
conda env list

# 测试环境激活
source /opt/miniconda3/etc/profile.d/conda.sh
conda activate isaaclab
python -c "import omni.isaac.lab"
```

 **解决方案** :

```bash
# 重新配置环境
./conda_setup.sh

# 或手动修复
conda activate isaaclab
pip install -e /opt/isaaclab
```

### 问题3: IsaacLab导入失败

 **现象** : Python无法导入omni.isaac.lab

 **排查步骤** :

```bash
# 检查IsaacLab路径
ls -la /opt/isaaclab

# 检查Python路径
conda activate isaaclab
python -c "import sys; print(sys.path)"

# 检查环境变量
echo $ISAACLAB_PATH
echo $PYTHONPATH
```

 **解决方案** :

```bash
# 设置环境变量
export ISAACLAB_PATH=/opt/isaaclab
export PYTHONPATH=$ISAACLAB_PATH:$PYTHONPATH

# 或重新安装IsaacLab
cd /opt/isaaclab
pip install -e .
```

### 问题4: TensorBoard无法访问

 **现象** : 点击TensorBoard链接无响应

 **排查步骤** :

```bash
# 检查TensorBoard进程
ps aux | grep tensorboard

# 检查端口占用
netstat -tulpn | grep 6006

# 手动启动测试
conda activate isaaclab
tensorboard --logdir ./outputs/test/logs --port 6006
```

### 问题5: 代码同步失败

 **现象** : rsync同步报错

 **排查步骤** :

```bash
# 检查rsync安装
rsync --version

# 测试同步命令
rsync -avz --dry-run /source/path/ /target/path/

# 检查权限
ls -la /opt/isaaclab/source
```

## 🔄 维护操作

### 日常维护

```bash
# 查看服务状态
sudo systemctl status isaaclab-manager

# 重启服务
sudo systemctl restart isaaclab-manager

# 查看日志
sudo journalctl -u isaaclab-manager -f

# 备份数据
./backup.sh
```

### 更新服务

```bash
# 停止服务
sudo systemctl stop isaaclab-manager

# 备份数据
./backup.sh

# 更新代码
git pull origin main

# 重新构建
cargo build --release

# 启动服务
sudo systemctl start isaaclab-manager
```

### 清理存储

```bash
# 清理旧的训练输出
find outputs/ -name "*.pt" -mtime +30 -delete

# 清理日志文件
find logs/ -name "*.log" -mtime +7 -delete

# 压缩大文件
gzip outputs/*/logs/*.log
```

## 📊 性能优化

### 系统资源监控

```bash
# 监控CPU和内存
htop

# 监控GPU使用
nvidia-smi

# 监控磁盘IO
iotop
```

### 数据库优化

```bash
# 优化SQLite数据库
sqlite3 data/isaaclab_manager.db "VACUUM;"

# 分析数据库大小
du -h data/isaaclab_manager.db
```

### 网络优化

```bash
# 配置防火墙
sudo ufw allow 3000/tcp
sudo ufw allow 6006:6020/tcp

# 配置反向代理(可选)
# 使用Nginx配置域名和SSL
```

## 🔐 安全配置

### 文件权限

```bash
# 设置正确的文件权限
chmod 700 data/
chmod 755 outputs/
chmod 644 config/app.toml
sudo chown -R $USER:$USER ./
```

### 网络安全

```bash
# 限制访问IP(可选)
# 在config/app.toml中设置host = "127.0.0.1"

# 使用Nginx反向代理
# 配置SSL证书和访问控制
```

## 📞 技术支持

### 获取帮助

1. **查看日志** : `sudo journalctl -u isaaclab-manager -f`
2. **检查配置** : `cat config/app.toml`
3. **测试环境** : `python test_environment.py`
4. **重新安装** : `./setup.sh`

### 联系方式

* **GitHub Issues** : 在项目仓库提交问题
* **文档** : 查看项目README和Wiki
* **社区** : 加入相关技术讨论群

## ✅ 验收测试

安装完成后，请执行以下测试确保系统正常工作：

### 1. 基础功能测试

```bash
# 服务状态测试
curl http://localhost:3000/api/tasks

# conda环境测试
curl http://localhost:3000/api/conda/envs
```

### 2. 完整训练测试

1. 创建一个简单的Cartpole任务
2. 选择正确的conda环境
3. 启动任务并查看日志
4. 检查TensorBoard输出
5. 验证ONNX文件生成

### 3. 代码同步测试

1. 创建测试代码目录
2. 配置同步源和目标
3. 执行同步操作
4. 验证文件传输

完成所有测试后，您的IsaacLab训练管理服务就已经准备就绪了！

---

🎉 **恭喜！您已成功安装IsaacLab训练管理服务！**

现在您可以通过现代化的Web界面轻松管理强化学习训练任务，享受高效的开发体验！
