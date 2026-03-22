# Alchemy 空间计算包管理 -- 实现指南

> 本文档详细描述如何为 Alchemy 包管理器添加空间计算 (Spatial Computing) 支持，使其能够统一管理 JavaScript/TypeScript 与 Swift 双依赖树，服务于基于 Vue 的 visionOS 空间应用项目。

---

## 1. 概述

### 1.1 背景与定位

Alchemy 是一个用 Rust 编写的 npm 兼容包管理器，采用 pnpm 风格的 `node_modules/` 布局，通过全局内容寻址存储 (`~/.alchemy-store/`) 进行硬链接。当前架构包含五个 workspace crate：

```
alchemy_cli      - CLI 入口（Clap 子命令：install, add, remove, init, run）
alchemy_core     - 核心类型（resolver, dependency, manifest, lockfile, graph, platform）
alchemy_registry - npm 注册表 HTTP/2 客户端
alchemy_store    - 内容寻址存储（SHA-256 完整性校验）
alchemy_linker   - pnpm 风格 node_modules 链接（硬链接 + 符号链接 + .bin）
```

空间计算场景下，Alchemy 需要承担一个新角色：管理**混合 JavaScript/TypeScript + Swift 依赖**的空间 Vue 项目。一个典型的空间 Vue 项目同时包含：

- **npm 依赖树**：Vue 3、Taraxacum（空间 Vue 框架）、Vite 插件等 JS/TS 包
- **Swift 依赖树**：RealityKit、SwiftUI、自定义 Swift Package 等原生 visionOS 依赖

这两棵依赖树目前由完全不同的工具链管理（npm/pnpm vs. Swift Package Manager），对开发者造成割裂体验。Alchemy 的目标是将两者统一到单一依赖解析流程中。

### 1.2 核心挑战

| 挑战 | 说明 |
|------|------|
| 双依赖图 | JS 依赖使用 semver（npm 语义），Swift 依赖使用 SPM 版本解析规则，两套解析逻辑需共存 |
| 清单文件格式 | 当前 Alchemy 读取 `package.json`（JSON），空间项目需要引入 TOML 格式的 `alchemy.toml` 同时声明两种依赖 |
| 平台约束 | Swift 依赖携带平台版本约束（`visionOS 2.0+`、`iOS 18+`），需要扩展现有的 `Platform` 类型 |
| 构建产物 | JS 产物是 `node_modules/` 中的包文件，Swift 产物是编译后的 framework/binary，链接方式完全不同 |
| 锁文件统一 | 单个 `alchemy.lock` 必须同时锁定 JS 和 Swift 依赖版本，保证确定性构建 |

---

## 2. 双依赖解析

### 2.1 清单文件扩展 (alchemy.toml)

引入 TOML 格式的统一清单文件 `alchemy.toml`，与现有 `package.json` 共存。当项目同时存在 `alchemy.toml` 和 `package.json` 时，`alchemy.toml` 为主清单，`package.json` 中的 JS 依赖自动合并。

#### 完整格式定义

```toml
[package]
name = "my-spatial-app"
version = "0.1.0"
description = "一个空间 Vue 应用"
license = "MIT"

# 标准 JS/TS 依赖（等同于 package.json 的 dependencies）
[dependencies]
vue = "^3.6"
taraxacum = "^1.0"
"@taraxacum/runtime" = "^1.0"

[dev-dependencies]
vite = "^6.0"
"vite-plugin-vue-spatial" = "^1.0"
typescript = "^5.5"

# Swift / 空间计算依赖
[spatial-dependencies]
RealityKit = { platform = "visionOS 2.0+" }
SwiftUI = { platform = "visionOS 2.0+" }
ARKit = { platform = "visionOS 2.0+" }
"custom-swift-pkg" = { url = "https://github.com/user/custom-swift-pkg.git", from = "1.0.0" }
"another-pkg" = { url = "https://github.com/user/another-pkg.git", branch = "main" }
"local-swift-pkg" = { path = "../local-swift-pkg" }

[spatial-dev-dependencies]
"swift-snapshot-testing" = { url = "https://github.com/pointfreeco/swift-snapshot-testing.git", from = "1.12.0" }

# 空间项目配置
[spatial]
swift-tools-version = "6.0"
platforms = ["visionOS 2.0+", "iOS 18.0+"]
bundle-identifier = "com.example.my-spatial-app"
deployment-target = "visionOS 2.0"
xcode-project-name = "MyApp"

# 场景定义
[[spatial.scenes]]
name = "MainWindow"
type = "window"
entry = "src/scenes/index.vue"
default-size = { width = 1280, height = 720 }

[[spatial.scenes]]
name = "ImmersiveView"
type = "immersive"
entry = "src/scenes/immersive.vue"
immersion-style = "mixed"
```

#### Rust 结构体定义

在 `alchemy_core` 中新增 `spatial_manifest.rs` 模块：

```rust
// crates/alchemy_core/src/spatial_manifest.rs

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// alchemy.toml 统一清单文件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlchemyManifest {
    pub package: PackageInfo,

    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,

    #[serde(default, rename = "dev-dependencies")]
    pub dev_dependencies: BTreeMap<String, String>,

    #[serde(default, rename = "spatial-dependencies")]
    pub spatial_dependencies: BTreeMap<String, SpatialDepSpec>,

    #[serde(default, rename = "spatial-dev-dependencies")]
    pub spatial_dev_dependencies: BTreeMap<String, SpatialDepSpec>,

    #[serde(default)]
    pub spatial: Option<SpatialConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
}

/// Swift 依赖规格 -- 支持多种来源
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SpatialDepSpec {
    /// 系统框架: RealityKit = { platform = "visionOS 2.0+" }
    SystemFramework {
        platform: String,
    },
    /// Git 仓库 (精确版本): { url = "...", from = "1.0.0" }
    GitVersion {
        url: String,
        from: String,
        #[serde(default)]
        platform: Option<String>,
    },
    /// Git 仓库 (分支): { url = "...", branch = "main" }
    GitBranch {
        url: String,
        branch: String,
        #[serde(default)]
        platform: Option<String>,
    },
    /// Git 仓库 (精确修订): { url = "...", revision = "abc123" }
    GitRevision {
        url: String,
        revision: String,
        #[serde(default)]
        platform: Option<String>,
    },
    /// 本地路径: { path = "../local-pkg" }
    LocalPath {
        path: PathBuf,
        #[serde(default)]
        platform: Option<String>,
    },
}

/// 空间项目配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpatialConfig {
    #[serde(rename = "swift-tools-version")]
    pub swift_tools_version: String,

    #[serde(default)]
    pub platforms: Vec<String>,

    #[serde(default, rename = "bundle-identifier")]
    pub bundle_identifier: Option<String>,

    #[serde(default, rename = "deployment-target")]
    pub deployment_target: Option<String>,

    #[serde(default, rename = "xcode-project-name")]
    pub xcode_project_name: Option<String>,

    #[serde(default)]
    pub scenes: Vec<SceneConfig>,
}

/// 场景定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneConfig {
    pub name: String,
    /// "window" | "immersive" | "volume"
    #[serde(rename = "type")]
    pub scene_type: SceneType,
    pub entry: String,
    #[serde(default, rename = "default-size")]
    pub default_size: Option<WindowSize>,
    #[serde(default, rename = "immersion-style")]
    pub immersion_style: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SceneType {
    Window,
    Immersive,
    Volume,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowSize {
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub depth: Option<u32>,
}

impl AlchemyManifest {
    /// 从 alchemy.toml 文件解析
    pub fn from_toml(content: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(content)
    }

    /// 从文件路径加载
    pub fn from_path(path: &Path) -> crate::error::AlchemyResult<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::from_toml(&content).map_err(|e| {
            crate::error::AlchemyError::ManifestParse(format!("{}: {}", path.display(), e))
        })
    }

    /// 是否包含空间依赖
    pub fn has_spatial_dependencies(&self) -> bool {
        !self.spatial_dependencies.is_empty()
            || !self.spatial_dev_dependencies.is_empty()
    }

    /// 所有 JS 依赖（合并 dependencies + dev-dependencies）
    pub fn all_js_dependencies(&self) -> BTreeMap<String, String> {
        let mut deps = self.dependencies.clone();
        deps.extend(self.dev_dependencies.clone());
        deps
    }

    /// 所有 Swift 依赖（合并 spatial-dependencies + spatial-dev-dependencies）
    pub fn all_spatial_dependencies(&self) -> BTreeMap<String, SpatialDepSpec> {
        let mut deps = self.spatial_dependencies.clone();
        deps.extend(self.spatial_dev_dependencies.clone());
        deps
    }
}
```

#### 需要在 alchemy_core 中注册新模块

```rust
// crates/alchemy_core/src/lib.rs  (增加一行)
pub mod spatial_manifest;
```

#### 新增 Cargo.toml 依赖

`alchemy_core/Cargo.toml` 需要添加 `toml` crate：

```toml
[dependencies]
toml = "0.8"
```

### 2.2 Swift 包解析

#### 与现有解析器的关系

当前的 `Resolver<F: MetadataFetcher>` 使用 `MetadataFetcher` trait 从 npm 注册表获取版本信息。Swift 依赖解析需要一个独立的解析流程，因为：

1. Swift 包不存在于 npm 注册表中
2. SPM 的版本解析语义与 npm semver 存在差异（如 `from: "1.0.0"` 对应 `>=1.0.0 <2.0.0`）
3. Swift 包的来源可以是 Git 仓库、本地路径或系统框架，不涉及 tarball 下载

解析策略：**两阶段解析 + 合并**。

```
Phase 1: Resolver<RegistryClient>.resolve(js_deps)      → JS ResolutionResult
Phase 2: SwiftResolver.resolve(spatial_deps)             → SwiftResolutionResult
Merge:   UnifiedResolution::merge(js_result, swift_result) → UnifiedResolutionResult
```

#### Swift 版本解析规则映射

| alchemy.toml 语法 | SPM 等效语法 | 含义 |
|---|---|---|
| `from = "1.0.0"` | `.upToNextMajor(from: "1.0.0")` | `>=1.0.0 <2.0.0` |
| `from = "1.2.0"` | `.upToNextMajor(from: "1.2.0")` | `>=1.2.0 <2.0.0` |
| `branch = "main"` | `.branch("main")` | 追踪分支最新 commit |
| `revision = "abc123"` | `.revision("abc123")` | 锁定到精确 commit |
| `path = "../pkg"` | `.package(path: "../pkg")` | 本地路径引用 |

#### SwiftResolver 核心结构

```rust
// crates/alchemy_spatial/src/swift_resolver.rs

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::platform::SpatialPlatform;

/// Swift 包的解析标识符
#[derive(Debug, Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct SwiftPackageId {
    pub name: String,
    pub resolved_version: SwiftResolvedVersion,
}

/// Swift 依赖解析后的版本信息
#[derive(Debug, Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub enum SwiftResolvedVersion {
    /// 语义化版本: "1.2.3"
    Semver(String),
    /// 分支 + commit hash
    Branch { branch: String, commit: String },
    /// 精确 commit
    Revision(String),
    /// 系统框架（无版本，由 SDK 提供）
    SystemFramework,
    /// 本地路径（无版本）
    LocalPath(PathBuf),
}

/// Swift 包解析结果
pub struct SwiftResolutionResult {
    /// 所有已解析的 Swift 包
    pub packages: BTreeMap<String, ResolvedSwiftPackage>,
    /// Swift 包之间的依赖关系
    pub dependency_edges: Vec<(String, String)>,
    /// 解析过程中产生的警告
    pub warnings: Vec<String>,
}

/// 已解析的 Swift 包
#[derive(Debug, Clone)]
pub struct ResolvedSwiftPackage {
    pub name: String,
    pub source: SwiftPackageSource,
    pub resolved_version: SwiftResolvedVersion,
    pub platform_constraints: Vec<SpatialPlatform>,
    /// 该包自身声明的 Swift 依赖（来自其 Package.swift）
    pub swift_dependencies: Vec<String>,
    /// 该包的 targets 列表
    pub targets: Vec<String>,
    /// 该包的 products 列表
    pub products: Vec<SwiftProduct>,
}

/// Swift 包来源
#[derive(Debug, Clone)]
pub enum SwiftPackageSource {
    /// Git 仓库
    Git { url: String },
    /// 本地路径
    Local { path: PathBuf },
    /// 系统框架
    System { framework_name: String },
}

/// Swift 包的 product 定义
#[derive(Debug, Clone)]
pub struct SwiftProduct {
    pub name: String,
    pub product_type: SwiftProductType,
}

#[derive(Debug, Clone)]
pub enum SwiftProductType {
    Library,
    Executable,
    Plugin,
}

/// Swift 依赖解析器
pub struct SwiftResolver {
    /// 缓存已 clone/fetch 的仓库路径
    repo_cache_dir: PathBuf,
    /// 是否允许网络访问（离线模式下为 false）
    allow_network: bool,
}

impl SwiftResolver {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            repo_cache_dir: cache_dir,
            allow_network: true,
        }
    }

    /// 解析所有 Swift 依赖
    ///
    /// 解析流程:
    /// 1. 区分系统框架和外部包
    /// 2. 对 Git 来源的包: clone/fetch 仓库到缓存目录
    /// 3. 读取每个包的 Package.swift 获取其传递依赖
    /// 4. 递归解析传递依赖
    /// 5. 版本冲突检测与报告
    pub async fn resolve(
        &self,
        spatial_deps: &BTreeMap<String, crate::SpatialDepSpec>,
    ) -> Result<SwiftResolutionResult, SwiftResolveError> {
        let mut result = SwiftResolutionResult {
            packages: BTreeMap::new(),
            dependency_edges: Vec::new(),
            warnings: Vec::new(),
        };

        for (name, spec) in spatial_deps {
            self.resolve_single(name, spec, &mut result).await?;
        }

        Ok(result)
    }

    /// 解析单个 Swift 依赖
    async fn resolve_single(
        &self,
        name: &str,
        spec: &crate::SpatialDepSpec,
        result: &mut SwiftResolutionResult,
    ) -> Result<(), SwiftResolveError> {
        // 跳过已解析的包
        if result.packages.contains_key(name) {
            return Ok(());
        }

        match spec {
            crate::SpatialDepSpec::SystemFramework { platform } => {
                // 系统框架无需 clone，直接记录
                result.packages.insert(name.to_string(), ResolvedSwiftPackage {
                    name: name.to_string(),
                    source: SwiftPackageSource::System {
                        framework_name: name.to_string(),
                    },
                    resolved_version: SwiftResolvedVersion::SystemFramework,
                    platform_constraints: vec![SpatialPlatform::parse(platform)],
                    swift_dependencies: Vec::new(),
                    targets: Vec::new(),
                    products: Vec::new(),
                });
            }
            crate::SpatialDepSpec::GitVersion { url, from, platform } => {
                // 1. Clone 或 fetch 仓库
                let repo_path = self.ensure_repo_cached(url).await?;
                // 2. 列出所有 tag，找到满足 from 约束的最高版本
                let resolved_tag = self.resolve_version_from_tags(&repo_path, from)?;
                // 3. Checkout 到该 tag，读取 Package.swift
                let package_swift = self.read_package_swift(&repo_path, &resolved_tag)?;
                // 4. 递归解析传递依赖
                self.resolve_transitive(&package_swift, result).await?;
                // 5. 记录解析结果
                result.packages.insert(name.to_string(), ResolvedSwiftPackage {
                    name: name.to_string(),
                    source: SwiftPackageSource::Git { url: url.clone() },
                    resolved_version: SwiftResolvedVersion::Semver(resolved_tag),
                    platform_constraints: platform.as_ref()
                        .map(|p| vec![SpatialPlatform::parse(p)])
                        .unwrap_or_default(),
                    swift_dependencies: package_swift.dependency_names(),
                    targets: package_swift.target_names(),
                    products: package_swift.products(),
                });
            }
            // GitBranch, GitRevision, LocalPath 的处理方式类似
            // ...（省略模式匹配的其他分支，实际实现需完整覆盖）
            _ => {}
        }

        Ok(())
    }

    /// 确保 Git 仓库已缓存到本地
    async fn ensure_repo_cached(&self, url: &str) -> Result<PathBuf, SwiftResolveError> {
        // 根据 URL 生成缓存路径: ~/.alchemy-store/swift-repos/{hash(url)}/
        let cache_key = sha256_short(url);
        let repo_path = self.repo_cache_dir.join("swift-repos").join(&cache_key);

        if repo_path.exists() {
            // git fetch 更新
            self.git_fetch(&repo_path).await?;
        } else if self.allow_network {
            // git clone --bare
            self.git_clone_bare(url, &repo_path).await?;
        } else {
            return Err(SwiftResolveError::NetworkDisabled(url.to_string()));
        }

        Ok(repo_path)
    }

    // ... 省略辅助方法的签名
}

/// Swift 解析错误
#[derive(Debug, thiserror::Error)]
pub enum SwiftResolveError {
    #[error("找不到满足约束 '{1}' 的版本: {0}")]
    VersionNotFound(String, String),

    #[error("无法解析 Package.swift: {0}")]
    PackageSwiftParse(String),

    #[error("Git 操作失败: {0}")]
    GitError(String),

    #[error("网络访问被禁用，无法获取: {0}")]
    NetworkDisabled(String),

    #[error("版本冲突: {0} 需要 {1}，但 {2} 已锁定")]
    VersionConflict(String, String, String),

    #[error("{0}")]
    Other(String),
}
```

### 2.3 统一锁文件

当前 Alchemy 使用 `alchemy-lock.yaml`（YAML 格式，pnpm 风格）。对于空间项目，扩展锁文件以包含 Swift 依赖信息。

#### 扩展后的锁文件格式

```yaml
lockfileVersion: "2.0"

# JS 依赖部分（与现有格式完全兼容）
importers:
  ".":
    dependencies:
      vue:
        specifier: "^3.6"
        version: "3.6.2"
      taraxacum:
        specifier: "^1.0"
        version: "1.0.5"
    devDependencies:
      vite:
        specifier: "^6.0"
        version: "6.1.0"

packages:
  /vue@3.6.2:
    resolution:
      integrity: "sha512-abc..."
      tarball: "https://registry.npmjs.org/vue/-/vue-3.6.2.tgz"
    dependencies:
      "@vue/runtime-dom": "3.6.2"
      "@vue/compiler-sfc": "3.6.2"
  # ...其他 JS 包

# Swift 依赖部分（新增）
spatialPackages:
  RealityKit:
    source: system
    platform: "visionOS 2.0+"

  custom-swift-pkg:
    source:
      git: "https://github.com/user/custom-swift-pkg.git"
    version: "1.2.3"
    commit: "a1b2c3d4e5f6..."
    dependencies:
      - "some-other-swift-pkg"

  another-pkg:
    source:
      git: "https://github.com/user/another-pkg.git"
    branch: "main"
    commit: "f6e5d4c3b2a1..."

  local-swift-pkg:
    source:
      path: "../local-swift-pkg"
```

#### 扩展 Lockfile 结构体

```rust
// 在 crates/alchemy_core/src/lockfile.rs 中扩展

/// 扩展后的锁文件，支持空间依赖
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lockfile {
    #[serde(rename = "lockfileVersion")]
    pub lockfile_version: String,
    pub importers: BTreeMap<String, LockfileImporter>,
    pub packages: BTreeMap<String, LockfilePackage>,

    // 新增：Swift/空间依赖锁定信息
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty", rename = "spatialPackages")]
    pub spatial_packages: BTreeMap<String, LockfileSpatialPackage>,
}

/// 锁文件中的 Swift 包条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockfileSpatialPackage {
    pub source: LockfileSpatialSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
}

/// Swift 包来源（锁文件中的序列化形式）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LockfileSpatialSource {
    System(String),  // "system"
    Git { git: String },
    Path { path: String },
}
```

---

## 3. 新 Crate: alchemy_spatial

### 3.1 目录结构

```
crates/alchemy_spatial/
  Cargo.toml
  src/
    lib.rs                 - 模块根，导出公共 API
    swift_resolver.rs      - Swift Package Manager 依赖解析
    swift_manifest.rs      - Package.swift 生成与解析
    platform.rs            - 平台约束处理（visionOS, iOS, macOS）
    xcode.rs               - Xcode 项目生成辅助
    bridge.rs              - JS <-> Swift 依赖桥接
    scaffold.rs            - 项目脚手架生成
    build.rs               - 构建编排（Vue 编译 + Swift 编译）
```

### 3.2 Cargo.toml

```toml
[package]
name = "alchemy_spatial"
version.workspace = true
edition.workspace = true

[dependencies]
alchemy_core = { path = "../alchemy_core" }
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
toml = "0.8"
regex = "1"
which = "7"          # 查找 swift, xcodebuild 等可执行文件
tempfile = "3"

[dev-dependencies]
tokio = { workspace = true, features = ["test-util"] }
```

需要在 workspace 根 `Cargo.toml` 的 `members` 中注册：

```toml
[workspace]
members = [
    "crates/alchemy_cli",
    "crates/alchemy_core",
    "crates/alchemy_registry",
    "crates/alchemy_store",
    "crates/alchemy_linker",
    "crates/alchemy_spatial",   # 新增
]
```

### 3.3 lib.rs -- 模块根

```rust
// crates/alchemy_spatial/src/lib.rs

pub mod bridge;
pub mod build;
pub mod platform;
pub mod scaffold;
pub mod swift_manifest;
pub mod swift_resolver;
pub mod xcode;

// 从 alchemy_core 重新导出空间清单类型
pub use alchemy_core::spatial_manifest::{
    AlchemyManifest, SceneConfig, SceneType, SpatialConfig, SpatialDepSpec,
};
```

### 3.4 platform.rs -- 平台约束处理

```rust
// crates/alchemy_spatial/src/platform.rs

use serde::{Deserialize, Serialize};

/// 空间计算目标平台
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SpatialPlatform {
    pub os: SpatialOS,
    pub minimum_version: String,
}

/// 支持的操作系统
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum SpatialOS {
    VisionOS,
    IOS,
    MacOS,
    TvOS,
    WatchOS,
}

impl SpatialPlatform {
    /// 解析平台字符串: "visionOS 2.0+", "iOS 18.0+", "macOS 15.0+"
    pub fn parse(s: &str) -> Self {
        let s = s.trim().trim_end_matches('+');
        let parts: Vec<&str> = s.splitn(2, ' ').collect();
        let os = match parts[0].to_lowercase().as_str() {
            "visionos" => SpatialOS::VisionOS,
            "ios" => SpatialOS::IOS,
            "macos" => SpatialOS::MacOS,
            "tvos" => SpatialOS::TvOS,
            "watchos" => SpatialOS::WatchOS,
            _ => SpatialOS::VisionOS, // 默认
        };
        let version = parts.get(1).unwrap_or(&"1.0").to_string();
        Self {
            os,
            minimum_version: version,
        }
    }

    /// 生成 Package.swift 中的 .platform() 表达式
    pub fn to_swift_platform_expr(&self) -> String {
        let os_name = match self.os {
            SpatialOS::VisionOS => ".visionOS",
            SpatialOS::IOS => ".iOS",
            SpatialOS::MacOS => ".macOS",
            SpatialOS::TvOS => ".tvOS",
            SpatialOS::WatchOS => ".watchOS",
        };
        format!("{}(.v{})", os_name, self.minimum_version.replace('.', "_"))
    }

    /// 检查当前开发环境是否支持目标平台
    pub fn is_sdk_available(&self) -> bool {
        // 检查 xcrun --sdk <platform> --show-sdk-path 是否成功
        let sdk_name = match self.os {
            SpatialOS::VisionOS => "xros",
            SpatialOS::IOS => "iphoneos",
            SpatialOS::MacOS => "macosx",
            SpatialOS::TvOS => "appletvos",
            SpatialOS::WatchOS => "watchos",
        };
        std::process::Command::new("xcrun")
            .args(["--sdk", sdk_name, "--show-sdk-path"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

impl std::fmt::Display for SpatialPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let os_name = match self.os {
            SpatialOS::VisionOS => "visionOS",
            SpatialOS::IOS => "iOS",
            SpatialOS::MacOS => "macOS",
            SpatialOS::TvOS => "tvOS",
            SpatialOS::WatchOS => "watchOS",
        };
        write!(f, "{} {}+", os_name, self.minimum_version)
    }
}
```

### 3.5 swift_manifest.rs -- Package.swift 生成

```rust
// crates/alchemy_spatial/src/swift_manifest.rs

use std::collections::BTreeMap;
use std::path::Path;

use crate::platform::SpatialPlatform;
use alchemy_core::spatial_manifest::{SpatialConfig, SpatialDepSpec};

/// 从 alchemy.toml 的 spatial 配置生成 Package.swift 文件内容
pub fn generate_package_swift(
    config: &SpatialConfig,
    spatial_deps: &BTreeMap<String, SpatialDepSpec>,
    package_name: &str,
) -> String {
    let mut lines = Vec::new();

    // swift-tools-version 声明
    lines.push(format!(
        "// swift-tools-version: {}",
        config.swift_tools_version
    ));
    lines.push(String::new());
    lines.push("import PackageDescription".to_string());
    lines.push(String::new());

    // platforms
    let platforms: Vec<String> = config
        .platforms
        .iter()
        .map(|p| SpatialPlatform::parse(p).to_swift_platform_expr())
        .collect();

    lines.push("let package = Package(".to_string());
    lines.push(format!("    name: \"{}\",", package_name));

    if !platforms.is_empty() {
        lines.push(format!(
            "    platforms: [{}],",
            platforms.join(", ")
        ));
    }

    // products
    lines.push("    products: [".to_string());
    lines.push(format!(
        "        .library(name: \"{}\", targets: [\"{}\"]),",
        package_name, package_name
    ));
    // 为每个场景生成 executable target
    for scene in &config.scenes {
        lines.push(format!(
            "        .executable(name: \"{}Scene\", targets: [\"{}Scene\"]),",
            scene.name, scene.name
        ));
    }
    lines.push("    ],".to_string());

    // dependencies
    lines.push("    dependencies: [".to_string());
    for (name, spec) in spatial_deps {
        if let Some(dep_line) = spec_to_swift_dependency(name, spec) {
            lines.push(format!("        {},", dep_line));
        }
    }
    lines.push("    ],".to_string());

    // targets
    lines.push("    targets: [".to_string());
    // 主 target
    let dep_product_refs: Vec<String> = spatial_deps
        .iter()
        .filter_map(|(name, spec)| spec_to_product_ref(name, spec))
        .collect();
    lines.push(format!("        .target("));
    lines.push(format!("            name: \"{}\",", package_name));
    lines.push(format!(
        "            dependencies: [{}],",
        dep_product_refs.join(", ")
    ));
    lines.push(format!("            path: \"Sources/{}\"", package_name));
    lines.push(format!("        ),"));

    // VueSpatialRuntime bridge target
    lines.push("        .target(".to_string());
    lines.push("            name: \"VueSpatialRuntime\",".to_string());
    lines.push("            dependencies: [],".to_string());
    lines.push("            path: \"Sources/VueSpatialRuntime\"".to_string());
    lines.push("        ),".to_string());

    lines.push("    ]".to_string());
    lines.push(")".to_string());
    lines.push(String::new());

    lines.join("\n")
}

/// 将 SpatialDepSpec 转换为 Package.swift 中的 .package() 声明
fn spec_to_swift_dependency(name: &str, spec: &SpatialDepSpec) -> Option<String> {
    match spec {
        SpatialDepSpec::SystemFramework { .. } => {
            // 系统框架不需要在 dependencies 中声明
            None
        }
        SpatialDepSpec::GitVersion { url, from, .. } => Some(format!(
            ".package(url: \"{}\", from: \"{}\")",
            url, from
        )),
        SpatialDepSpec::GitBranch { url, branch, .. } => Some(format!(
            ".package(url: \"{}\", branch: \"{}\")",
            url, branch
        )),
        SpatialDepSpec::GitRevision { url, revision, .. } => Some(format!(
            ".package(url: \"{}\", revision: \"{}\")",
            url, revision
        )),
        SpatialDepSpec::LocalPath { path, .. } => Some(format!(
            ".package(name: \"{}\", path: \"{}\")",
            name,
            path.display()
        )),
    }
}

/// 将 SpatialDepSpec 转换为 target dependencies 中的 product 引用
fn spec_to_product_ref(name: &str, spec: &SpatialDepSpec) -> Option<String> {
    match spec {
        SpatialDepSpec::SystemFramework { .. } => {
            // 系统框架直接按名称引用
            Some(format!("\"{}\"", name))
        }
        _ => {
            // 外部包需要 .product() 引用
            Some(format!(".product(name: \"{}\", package: \"{}\")", name, name))
        }
    }
}

/// 从已有 Package.swift 文件中解析依赖信息（用于读取第三方包的传递依赖）
///
/// 实现策略:
/// 方案 A: 正则表达式解析 -- 快速但不完整
/// 方案 B: 调用 `swift package dump-package` 得到 JSON -- 准确但需要 Swift 工具链
///
/// 推荐方案 B，因为 Package.swift 是可执行的 Swift 代码，
/// 静态正则无法处理条件编译等高级用法。
pub fn parse_package_swift(package_swift_dir: &Path) -> Result<ParsedSwiftPackage, String> {
    let output = std::process::Command::new("swift")
        .args(["package", "dump-package"])
        .current_dir(package_swift_dir)
        .output()
        .map_err(|e| format!("无法执行 swift package dump-package: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("swift package dump-package 失败: {}", stderr));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("解析 dump-package JSON 失败: {}", e))?;

    // 从 JSON 中提取依赖、targets、products 信息
    Ok(extract_package_info(&json))
}

/// 从 dump-package JSON 中提取包信息
fn extract_package_info(json: &serde_json::Value) -> ParsedSwiftPackage {
    let name = json["name"].as_str().unwrap_or("").to_string();

    let dependencies: Vec<String> = json["dependencies"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|d| {
                    d["sourceControl"].as_array().and_then(|sc| {
                        sc.first().and_then(|s| {
                            s["identity"].as_str().map(|s| s.to_string())
                        })
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let targets: Vec<String> = json["targets"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t["name"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    ParsedSwiftPackage {
        name,
        dependency_names: dependencies,
        target_names: targets,
    }
}

/// 从 Package.swift 解析出的结构
#[derive(Debug, Clone)]
pub struct ParsedSwiftPackage {
    pub name: String,
    pub dependency_names: Vec<String>,
    pub target_names: Vec<String>,
}

impl ParsedSwiftPackage {
    pub fn dependency_names(&self) -> Vec<String> {
        self.dependency_names.clone()
    }
    pub fn target_names(&self) -> Vec<String> {
        self.target_names.clone()
    }
    pub fn products(&self) -> Vec<crate::swift_resolver::SwiftProduct> {
        // 简化实现: 将每个 target 映射为一个 library product
        self.target_names
            .iter()
            .map(|t| crate::swift_resolver::SwiftProduct {
                name: t.clone(),
                product_type: crate::swift_resolver::SwiftProductType::Library,
            })
            .collect()
    }
}
```

### 3.6 bridge.rs -- JS 与 Swift 依赖桥接

```rust
// crates/alchemy_spatial/src/bridge.rs

use std::collections::BTreeMap;

use alchemy_core::resolver::ResolutionResult;
use crate::swift_resolver::SwiftResolutionResult;

/// 统一的解析结果，合并 JS 和 Swift 依赖树
pub struct UnifiedResolutionResult {
    /// JS 依赖解析结果
    pub js: ResolutionResult,
    /// Swift 依赖解析结果
    pub swift: SwiftResolutionResult,
    /// JS 包对 Swift 包的桥接依赖
    /// 例如: taraxacum (JS) -> RealityKit (Swift) 的运行时桥接
    pub bridge_edges: Vec<BridgeDependency>,
}

/// JS <-> Swift 桥接依赖
#[derive(Debug, Clone)]
pub struct BridgeDependency {
    /// JS 包名
    pub js_package: String,
    /// Swift 包名
    pub swift_package: String,
    /// 桥接类型
    pub bridge_type: BridgeType,
}

#[derive(Debug, Clone)]
pub enum BridgeType {
    /// JS 包在运行时需要调用 Swift 框架
    RuntimeBinding,
    /// JS 包生成的代码需要在 Swift 端编译
    CodeGeneration,
    /// JS 包中的 Vue SFC 被转译为引用 Swift 类型的代码
    SpatialComponent,
}

/// 合并两棵依赖树
///
/// 合并逻辑:
/// 1. 检测桥接依赖: 某些 JS 包（如 taraxacum）会在 package.json 中声明
///    "spatialPeerDependencies" 字段，指明其需要的 Swift 包
/// 2. 验证桥接兼容性: 确保 JS 包需要的 Swift 框架在 spatial-dependencies 中已声明
/// 3. 生成桥接边: 记录 JS->Swift 的运行时依赖关系
pub fn merge_resolutions(
    js: ResolutionResult,
    swift: SwiftResolutionResult,
) -> UnifiedResolutionResult {
    let bridge_edges = detect_bridge_dependencies(&js, &swift);

    UnifiedResolutionResult {
        js,
        swift,
        bridge_edges,
    }
}

/// 检测 JS 和 Swift 之间的桥接依赖
fn detect_bridge_dependencies(
    js: &ResolutionResult,
    swift: &SwiftResolutionResult,
) -> Vec<BridgeDependency> {
    let mut bridges = Vec::new();

    // 已知的桥接包列表（硬编码 + package.json 中的声明）
    let known_bridges: BTreeMap<&str, Vec<&str>> = BTreeMap::from([
        ("taraxacum", vec!["RealityKit", "SwiftUI"]),
        ("@taraxacum/runtime", vec!["RealityKit", "SwiftUI", "ARKit"]),
    ]);

    for (js_pkg, required_swift) in &known_bridges {
        if js.packages.keys().any(|id| id.name == *js_pkg) {
            for swift_pkg in required_swift {
                if swift.packages.contains_key(*swift_pkg) {
                    bridges.push(BridgeDependency {
                        js_package: js_pkg.to_string(),
                        swift_package: swift_pkg.to_string(),
                        bridge_type: BridgeType::RuntimeBinding,
                    });
                }
            }
        }
    }

    bridges
}
```

### 3.7 xcode.rs -- Xcode 项目生成

```rust
// crates/alchemy_spatial/src/xcode.rs

use std::path::Path;
use alchemy_core::spatial_manifest::SpatialConfig;

/// Xcode 项目生成器
///
/// 生成的项目结构:
///   xcode/
///     MyApp.xcodeproj/
///       project.pbxproj     <- 自动生成，不建议手动修改
///     Sources/
///       App.swift           <- visionOS 应用入口
///       VueSpatialRuntime/  <- JS-Swift 桥接运行时
///         Runtime.swift
///         SceneManager.swift
///         ComponentBridge.swift
pub struct XcodeGenerator<'a> {
    config: &'a SpatialConfig,
    project_dir: &'a Path,
    package_name: &'a str,
}

impl<'a> XcodeGenerator<'a> {
    pub fn new(config: &'a SpatialConfig, project_dir: &'a Path, package_name: &'a str) -> Self {
        Self {
            config,
            project_dir,
            package_name,
        }
    }

    /// 生成或更新 Xcode 项目
    ///
    /// 实现策略:
    /// 不直接操作 pbxproj（格式极其复杂），而是:
    /// 1. 生成 Package.swift（已有 swift_manifest 模块处理）
    /// 2. 使用 `swift package generate-xcodeproj`（已弃用）
    ///    或者更好的方案: 直接使用 xcodebuild 配合 Package.swift
    /// 3. 生成 Swift 源文件模板
    pub fn generate(&self) -> Result<(), XcodeGenError> {
        let xcode_dir = self.project_dir.join("xcode");
        let sources_dir = xcode_dir.join("Sources");

        std::fs::create_dir_all(&sources_dir)?;

        // 生成 App.swift
        self.generate_app_swift(&sources_dir)?;
        // 生成 VueSpatialRuntime 桥接框架
        self.generate_runtime_bridge(&sources_dir)?;

        Ok(())
    }

    /// 生成 visionOS 应用入口 App.swift
    fn generate_app_swift(&self, sources_dir: &Path) -> Result<(), XcodeGenError> {
        let app_name = self
            .config
            .xcode_project_name
            .as_deref()
            .unwrap_or(self.package_name);

        let mut swift_code = String::new();
        swift_code.push_str("import SwiftUI\n");
        swift_code.push_str("import RealityKit\n");
        swift_code.push_str("import VueSpatialRuntime\n");
        swift_code.push_str("\n");
        swift_code.push_str("@main\n");
        swift_code.push_str(&format!("struct {}App: App {{\n", app_name));
        swift_code.push_str("    @State private var runtime = VueSpatialRuntime()\n");
        swift_code.push_str("\n");
        swift_code.push_str("    var body: some Scene {\n");

        for scene in &self.config.scenes {
            match scene.scene_type {
                alchemy_core::spatial_manifest::SceneType::Window => {
                    swift_code.push_str(&format!(
                        "        WindowGroup(id: \"{}\") {{\n",
                        scene.name
                    ));
                    swift_code.push_str(&format!(
                        "            VueSpatialView(component: \"{}\")\n",
                        scene.entry
                    ));
                    swift_code.push_str(
                        "                .environment(\\.vueSpatialRuntime, runtime)\n",
                    );
                    swift_code.push_str("        }\n");
                    if let Some(size) = &scene.default_size {
                        swift_code.push_str(&format!(
                            "        .defaultSize(width: {}, height: {})\n",
                            size.width, size.height
                        ));
                    }
                }
                alchemy_core::spatial_manifest::SceneType::Immersive => {
                    swift_code.push_str(&format!(
                        "        ImmersiveSpace(id: \"{}\") {{\n",
                        scene.name
                    ));
                    swift_code.push_str(&format!(
                        "            VueSpatialImmersiveView(component: \"{}\")\n",
                        scene.entry
                    ));
                    swift_code.push_str(
                        "                .environment(\\.vueSpatialRuntime, runtime)\n",
                    );
                    swift_code.push_str("        }\n");
                    if let Some(style) = &scene.immersion_style {
                        swift_code.push_str(&format!(
                            "        .immersionStyle(selection: .constant(.{}), in: .{})\n",
                            style, style
                        ));
                    }
                }
                alchemy_core::spatial_manifest::SceneType::Volume => {
                    swift_code.push_str(&format!(
                        "        WindowGroup(id: \"{}\") {{\n",
                        scene.name
                    ));
                    swift_code.push_str(&format!(
                        "            VueSpatialVolumeView(component: \"{}\")\n",
                        scene.entry
                    ));
                    swift_code.push_str(
                        "                .environment(\\.vueSpatialRuntime, runtime)\n",
                    );
                    swift_code.push_str("        }\n");
                    swift_code.push_str("        .windowStyle(.volumetric)\n");
                }
            }
            swift_code.push_str("\n");
        }

        swift_code.push_str("    }\n");
        swift_code.push_str("}\n");

        std::fs::write(sources_dir.join("App.swift"), swift_code)?;
        Ok(())
    }

    /// 生成 VueSpatialRuntime 桥接框架模板
    fn generate_runtime_bridge(&self, sources_dir: &Path) -> Result<(), XcodeGenError> {
        let runtime_dir = sources_dir.join("VueSpatialRuntime");
        std::fs::create_dir_all(&runtime_dir)?;

        // Runtime.swift - JS 引擎初始化与 Vue 组件运行时
        let runtime_swift = r#"import JavaScriptCore
import SwiftUI
import RealityKit

/// Vue Spatial 运行时 -- 负责 JS 执行环境与 Swift 桥接
@Observable
public class VueSpatialRuntime {
    private let jsContext: JSContext
    private var componentRegistry: [String: Any] = [:]

    public init() {
        self.jsContext = JSContext()!
        setupBridge()
    }

    private func setupBridge() {
        // 将 Swift API 注入到 JS 上下文
        // 由 alchemy spatial build 自动生成的桥接代码补充
    }

    /// 加载并执行 Vue bundle
    public func loadBundle(at path: String) throws {
        guard let script = try? String(contentsOfFile: path, encoding: .utf8) else {
            throw RuntimeError.bundleNotFound(path)
        }
        jsContext.evaluateScript(script)
    }

    public enum RuntimeError: Error {
        case bundleNotFound(String)
        case componentNotFound(String)
        case bridgeError(String)
    }
}
"#;
        std::fs::write(runtime_dir.join("Runtime.swift"), runtime_swift)?;

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum XcodeGenError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("Xcode 工具链未找到: {0}")]
    ToolchainMissing(String),
    #[error("项目生成失败: {0}")]
    GenerationFailed(String),
}
```

### 3.8 scaffold.rs -- 项目脚手架

```rust
// crates/alchemy_spatial/src/scaffold.rs

use std::path::Path;

/// 空间项目初始化器
pub struct SpatialScaffold<'a> {
    project_dir: &'a Path,
    project_name: &'a str,
}

impl<'a> SpatialScaffold<'a> {
    pub fn new(project_dir: &'a Path, project_name: &'a str) -> Self {
        Self {
            project_dir,
            project_name,
        }
    }

    /// 创建完整的空间项目结构
    ///
    /// 生成的结构:
    ///   my-spatial-app/
    ///     alchemy.toml              # 统一清单
    ///     package.json              # 兼容 npm 工具
    ///     vite.config.ts            # Vite 构建配置
    ///     tsconfig.json             # TypeScript 配置
    ///     src/
    ///       App.vue                 # 根空间组件
    ///       scenes/
    ///         index.vue             # 默认窗口场景
    ///     Package.swift             # 由 alchemy 自动生成
    ///     xcode/
    ///       Sources/
    ///         App.swift             # visionOS 入口
    ///         VueSpatialRuntime/    # 桥接运行时
    pub fn scaffold(&self) -> Result<(), ScaffoldError> {
        // 创建目录结构
        let dirs = [
            "",
            "src",
            "src/scenes",
            "src/components",
            "src/composables",
            "xcode/Sources",
            "xcode/Sources/VueSpatialRuntime",
            "public",
        ];
        for dir in &dirs {
            std::fs::create_dir_all(self.project_dir.join(dir))?;
        }

        // 1. alchemy.toml
        self.write_alchemy_toml()?;
        // 2. package.json
        self.write_package_json()?;
        // 3. vite.config.ts
        self.write_vite_config()?;
        // 4. tsconfig.json
        self.write_tsconfig()?;
        // 5. src/App.vue
        self.write_app_vue()?;
        // 6. src/scenes/index.vue
        self.write_default_scene()?;
        // 7. .gitignore
        self.write_gitignore()?;

        Ok(())
    }

    fn write_alchemy_toml(&self) -> Result<(), ScaffoldError> {
        let content = format!(
            r#"[package]
name = "{name}"
version = "0.1.0"

[dependencies]
vue = "^3.6"
taraxacum = "^1.0"

[dev-dependencies]
vite = "^6.0"
"vite-plugin-vue-spatial" = "^1.0"
typescript = "^5.5"

[spatial-dependencies]
RealityKit = {{ platform = "visionOS 2.0+" }}
SwiftUI = {{ platform = "visionOS 2.0+" }}

[spatial]
swift-tools-version = "6.0"
platforms = ["visionOS 2.0+"]
bundle-identifier = "com.example.{name}"
deployment-target = "visionOS 2.0"
xcode-project-name = "{pascal_name}"

[[spatial.scenes]]
name = "MainWindow"
type = "window"
entry = "src/scenes/index.vue"
default-size = {{ width = 1280, height = 720 }}
"#,
            name = self.project_name,
            pascal_name = to_pascal_case(self.project_name),
        );
        std::fs::write(self.project_dir.join("alchemy.toml"), content)?;
        Ok(())
    }

    fn write_package_json(&self) -> Result<(), ScaffoldError> {
        let content = format!(
            r#"{{
  "name": "{}",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {{
    "dev": "alchemy spatial run --watch",
    "build": "alchemy spatial build",
    "preview": "alchemy spatial run"
  }}
}}
"#,
            self.project_name
        );
        std::fs::write(self.project_dir.join("package.json"), content)?;
        Ok(())
    }

    fn write_vite_config(&self) -> Result<(), ScaffoldError> {
        let content = r#"import { defineConfig } from 'vite'
import vueSpatial from 'vite-plugin-vue-spatial'

export default defineConfig({
  plugins: [
    vueSpatial({
      // 将 Vue SFC 中的 <spatial> 块编译为 RealityKit 实体
      spatialBlocks: true,
      // 生成 Swift 桥接代码
      bridgeCodegen: true,
    }),
  ],
  build: {
    // 输出到 xcode 资源目录
    outDir: 'xcode/Resources/bundle',
    // 空间 Vue 使用自定义运行时
    rollupOptions: {
      external: ['@taraxacum/native-bridge'],
    },
  },
})
"#;
        std::fs::write(self.project_dir.join("vite.config.ts"), content)?;
        Ok(())
    }

    fn write_tsconfig(&self) -> Result<(), ScaffoldError> {
        let content = r#"{
  "compilerOptions": {
    "target": "ESNext",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true,
    "jsx": "preserve",
    "types": ["taraxacum/spatial"],
    "paths": {
      "@/*": ["./src/*"]
    }
  },
  "include": ["src/**/*.ts", "src/**/*.vue"]
}
"#;
        std::fs::write(self.project_dir.join("tsconfig.json"), content)?;
        Ok(())
    }

    fn write_app_vue(&self) -> Result<(), ScaffoldError> {
        let content = r#"<script setup lang="ts">
import { useSpatialApp } from 'taraxacum'

const app = useSpatialApp()
</script>

<template>
  <spatial-root>
    <router-view />
  </spatial-root>
</template>
"#;
        std::fs::write(self.project_dir.join("src/App.vue"), content)?;
        Ok(())
    }

    fn write_default_scene(&self) -> Result<(), ScaffoldError> {
        let content = r#"<script setup lang="ts">
import { ref } from 'vue'
import { useScene, useRealityView } from 'taraxacum'

const scene = useScene()
const count = ref(0)

const { addEntity } = useRealityView()
</script>

<template>
  <spatial-window title="Welcome">
    <spatial-text :value="`Count: ${count}`" />
    <spatial-button @tap="count++">
      Increment
    </spatial-button>
  </spatial-window>
</template>

<spatial lang="realitykit">
  <!-- 声明式 RealityKit 实体，由编译器转为 Swift 代码 -->
  <Model name="decorative-sphere"
         mesh="sphere(radius: 0.1)"
         material="SimpleMaterial(color: .blue, isMetallic: true)"
         position="[0, 0.5, -1]" />
</spatial>
"#;
        std::fs::write(
            self.project_dir.join("src/scenes/index.vue"),
            content,
        )?;
        Ok(())
    }

    fn write_gitignore(&self) -> Result<(), ScaffoldError> {
        let content = r#"node_modules/
dist/
.alchemy-store/
*.xcodeproj/
xcode/build/
xcode/Resources/bundle/
.DS_Store
Package.resolved
"#;
        std::fs::write(self.project_dir.join(".gitignore"), content)?;
        Ok(())
    }
}

/// 将 kebab-case 转为 PascalCase
fn to_pascal_case(s: &str) -> String {
    s.split('-')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().chain(chars).collect(),
            }
        })
        .collect()
}

#[derive(Debug, thiserror::Error)]
pub enum ScaffoldError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("目录已存在且非空: {0}")]
    DirectoryNotEmpty(String),
}
```

### 3.9 build.rs -- 构建编排

```rust
// crates/alchemy_spatial/src/build.rs

use std::path::Path;
use std::process::Command;

/// 空间项目构建器
///
/// 构建流程:
/// 1. Vite 编译: Vue SFC -> JS bundle + 生成的 Swift 桥接代码
/// 2. Package.swift 同步: 从 alchemy.toml 重新生成 Package.swift
/// 3. Swift 编译: xcodebuild 编译 Swift 代码 + 链接框架
/// 4. 打包: 将 JS bundle 嵌入 .app 资源目录
pub struct SpatialBuilder<'a> {
    project_dir: &'a Path,
    /// 是否监视文件变更
    watch: bool,
    /// 目标平台
    destination: BuildDestination,
}

pub enum BuildDestination {
    VisionOSSimulator,
    VisionOSDevice,
    MacCatalyst,
}

impl<'a> SpatialBuilder<'a> {
    pub fn new(project_dir: &'a Path) -> Self {
        Self {
            project_dir,
            watch: false,
            destination: BuildDestination::VisionOSSimulator,
        }
    }

    pub fn with_watch(mut self, watch: bool) -> Self {
        self.watch = watch;
        self
    }

    pub fn with_destination(mut self, dest: BuildDestination) -> Self {
        self.destination = dest;
        self
    }

    /// 执行完整构建
    pub async fn build(&self) -> Result<BuildResult, BuildError> {
        tracing::info!("开始空间项目构建...");

        // Phase 1: Vite 构建
        tracing::info!("[1/4] 编译 Vue 组件...");
        self.run_vite_build()?;

        // Phase 2: 同步 Package.swift
        tracing::info!("[2/4] 同步 Package.swift...");
        self.sync_package_swift()?;

        // Phase 3: Swift 编译
        tracing::info!("[3/4] 编译 Swift 代码...");
        self.run_swift_build()?;

        // Phase 4: 打包
        tracing::info!("[4/4] 打包应用...");
        let app_path = self.package_app()?;

        Ok(BuildResult {
            app_path,
            js_bundle_size: 0, // 待实现
            swift_compile_time: std::time::Duration::default(),
        })
    }

    /// 构建并在模拟器中运行
    pub async fn run(&self) -> Result<(), BuildError> {
        let result = self.build().await?;

        // 启动 visionOS 模拟器
        let destination = match self.destination {
            BuildDestination::VisionOSSimulator => {
                "platform=visionOS Simulator,name=Apple Vision Pro"
            }
            BuildDestination::VisionOSDevice => "platform=visionOS,name=My Device",
            BuildDestination::MacCatalyst => "platform=macOS,variant=Mac Catalyst",
        };

        let status = Command::new("xcodebuild")
            .args([
                "-scheme",
                "MyApp",
                "-destination",
                destination,
                "run",
            ])
            .current_dir(self.project_dir)
            .status()?;

        if !status.success() {
            return Err(BuildError::XcodeBuildFailed(
                "xcodebuild run 失败".to_string(),
            ));
        }

        Ok(())
    }

    fn run_vite_build(&self) -> Result<(), BuildError> {
        let npx = if cfg!(target_os = "windows") {
            "npx.cmd"
        } else {
            "npx"
        };

        let status = Command::new(npx)
            .args(["vite", "build"])
            .current_dir(self.project_dir)
            .status()?;

        if !status.success() {
            return Err(BuildError::ViteBuildFailed);
        }
        Ok(())
    }

    fn sync_package_swift(&self) -> Result<(), BuildError> {
        // 读取 alchemy.toml，重新生成 Package.swift
        let manifest_path = self.project_dir.join("alchemy.toml");
        let manifest = alchemy_core::spatial_manifest::AlchemyManifest::from_path(&manifest_path)
            .map_err(|e| BuildError::ManifestError(e.to_string()))?;

        if let Some(ref config) = manifest.spatial {
            let package_swift = crate::swift_manifest::generate_package_swift(
                config,
                &manifest.spatial_dependencies,
                &manifest.package.name,
            );
            std::fs::write(self.project_dir.join("Package.swift"), package_swift)?;
        }

        Ok(())
    }

    fn run_swift_build(&self) -> Result<(), BuildError> {
        let status = Command::new("swift")
            .args(["build"])
            .current_dir(self.project_dir)
            .status()?;

        if !status.success() {
            return Err(BuildError::SwiftBuildFailed);
        }
        Ok(())
    }

    fn package_app(&self) -> Result<std::path::PathBuf, BuildError> {
        // 将 JS bundle 复制到 .app 资源目录
        let bundle_src = self.project_dir.join("xcode/Resources/bundle");
        let app_dir = self.project_dir.join("xcode/build/MyApp.app");
        let bundle_dest = app_dir.join("Contents/Resources/vue-bundle");

        if bundle_src.exists() {
            if bundle_dest.exists() {
                std::fs::remove_dir_all(&bundle_dest)?;
            }
            copy_dir_recursive(&bundle_src, &bundle_dest)?;
        }

        Ok(app_dir)
    }
}

/// 递归复制目录
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

pub struct BuildResult {
    pub app_path: std::path::PathBuf,
    pub js_bundle_size: u64,
    pub swift_compile_time: std::time::Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("Vite 构建失败")]
    ViteBuildFailed,
    #[error("Swift 编译失败")]
    SwiftBuildFailed,
    #[error("xcodebuild 失败: {0}")]
    XcodeBuildFailed(String),
    #[error("清单文件错误: {0}")]
    ManifestError(String),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
}
```

---

## 4. CLI 命令设计

### 4.1 命令结构扩展

在 `alchemy_cli` 中扩展 `Commands` 枚举：

```rust
// crates/alchemy_cli/src/main.rs -- 扩展后

#[derive(Subcommand)]
enum Commands {
    /// Install dependencies from package.json / alchemy.toml
    Install,
    /// Add a package to dependencies
    Add {
        /// Package name (optionally with version: pkg@version)
        package: String,
        /// Add as dev dependency
        #[arg(short = 'D', long)]
        dev: bool,
        /// Add as spatial (Swift) dependency
        #[arg(long)]
        spatial: bool,
    },
    /// Remove a package from dependencies
    Remove {
        /// Package name to remove
        package: String,
    },
    /// Initialize a new package.json
    Init,
    /// Run a script defined in package.json
    Run {
        /// Script name to run
        script: String,
        /// Additional arguments to pass to the script
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Spatial computing project commands
    Spatial {
        #[command(subcommand)]
        command: SpatialCommands,
    },
}

#[derive(Subcommand)]
enum SpatialCommands {
    /// Initialize a new spatial Vue project
    Init {
        /// Project name (defaults to current directory name)
        #[arg(default_value = ".")]
        name: String,
    },
    /// Build the spatial project (Vue compilation + Swift build)
    Build {
        /// Build for release
        #[arg(long)]
        release: bool,
        /// Target destination
        #[arg(long, default_value = "simulator")]
        destination: String,
    },
    /// Build and run in visionOS Simulator
    Run {
        /// Watch for file changes and rebuild
        #[arg(long)]
        watch: bool,
    },
    /// Generate .xcarchive / .ipa for distribution
    Package {
        /// Export method: development, ad-hoc, app-store
        #[arg(long, default_value = "development")]
        export_method: String,
    },
    /// Sync Package.swift from alchemy.toml
    Sync,
    /// Check spatial project setup and toolchain
    Doctor,
}
```

### 4.2 命令行交互示例

```bash
# 初始化空间项目
$ alchemy spatial init my-spatial-app
Creating spatial Vue project: my-spatial-app/
  alchemy.toml          - Unified manifest
  package.json          - npm compatibility
  vite.config.ts        - Vite + vue-spatial plugin
  tsconfig.json         - TypeScript config
  src/App.vue           - Root spatial component
  src/scenes/index.vue  - Default window scene

Done! Next steps:
  cd my-spatial-app
  alchemy install
  alchemy spatial run

# 安装所有依赖（JS + Swift）
$ alchemy install
Installing 12 JS dependencies...
  Resolved 47 packages
  Downloaded 3 new packages
  Linked
Resolving 2 Swift dependencies...
  RealityKit (system framework, visionOS 2.0+)
  SwiftUI (system framework, visionOS 2.0+)
Syncing Package.swift...

Done in 2.34s - 47 JS packages + 2 Swift packages installed

# 添加 Swift 空间依赖
$ alchemy add --spatial custom-swift-pkg --url https://github.com/user/pkg.git --from 1.0.0
Added custom-swift-pkg@^1.0.0 to [spatial-dependencies]
Resolving Swift dependencies...
  custom-swift-pkg 1.2.3 (from https://github.com/user/pkg.git)
Syncing Package.swift...

# 构建并运行
$ alchemy spatial run
[1/4] Compiling Vue components...
[2/4] Syncing Package.swift...
[3/4] Compiling Swift code...
[4/4] Packaging app...
Launching visionOS Simulator...

# 检查工具链
$ alchemy spatial doctor
Checking spatial development environment...
  Xcode 16.0+          OK (16.2)
  visionOS SDK         OK (2.1)
  Swift 6.0+           OK (6.0.2)
  Node.js 20+          OK (22.4.0)
  Vite                 OK (6.1.0)
All checks passed!
```

### 4.3 alchemy add --spatial 的实现

```rust
// crates/alchemy_cli/src/commands/add.rs -- 扩展 spatial 支持

/// 添加 Swift 空间依赖到 alchemy.toml
pub async fn run_spatial_add(
    package: &str,
    url: Option<&str>,
    from: Option<&str>,
    branch: Option<&str>,
    platform: Option<&str>,
) -> anyhow::Result<()> {
    let project_dir = std::env::current_dir()?;
    let manifest_path = project_dir.join("alchemy.toml");

    if !manifest_path.exists() {
        anyhow::bail!("No alchemy.toml found. Run `alchemy spatial init` first.");
    }

    // 读取现有 alchemy.toml
    let content = std::fs::read_to_string(&manifest_path)?;
    let mut manifest: toml::Value = content.parse()?;

    // 构造依赖条目
    let dep_value = if let Some(url) = url {
        if let Some(from_ver) = from {
            toml::Value::Table({
                let mut t = toml::map::Map::new();
                t.insert("url".to_string(), toml::Value::String(url.to_string()));
                t.insert("from".to_string(), toml::Value::String(from_ver.to_string()));
                if let Some(p) = platform {
                    t.insert("platform".to_string(), toml::Value::String(p.to_string()));
                }
                t
            })
        } else if let Some(br) = branch {
            toml::Value::Table({
                let mut t = toml::map::Map::new();
                t.insert("url".to_string(), toml::Value::String(url.to_string()));
                t.insert("branch".to_string(), toml::Value::String(br.to_string()));
                t
            })
        } else {
            anyhow::bail!("Git dependency requires --from or --branch");
        }
    } else {
        // 系统框架
        let p = platform.unwrap_or("visionOS 2.0+");
        toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert("platform".to_string(), toml::Value::String(p.to_string()));
            t
        })
    };

    // 插入到 [spatial-dependencies]
    let spatial_deps = manifest
        .as_table_mut()
        .unwrap()
        .entry("spatial-dependencies")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));

    spatial_deps
        .as_table_mut()
        .unwrap()
        .insert(package.to_string(), dep_value);

    // 写回文件
    let updated = toml::to_string_pretty(&manifest)?;
    std::fs::write(&manifest_path, updated)?;

    println!("Added {} to [spatial-dependencies]", package);

    // 重新同步 Package.swift
    // sync_package_swift(&project_dir)?;

    Ok(())
}
```

---

## 5. 项目脚手架

### 5.1 完整的生成文件结构

`alchemy spatial init my-spatial-app` 生成以下结构：

```
my-spatial-app/
  alchemy.toml                    # 统一清单（JS + Swift 依赖）
  alchemy.lock                    # 统一锁文件（初始为空，install 后生成）
  package.json                    # npm 兼容（scripts 指向 alchemy 命令）
  Package.swift                   # 自动生成，受 alchemy.toml 驱动
  vite.config.ts                  # Vite 配置 + vite-plugin-vue-spatial
  tsconfig.json                   # TypeScript 配置
  .gitignore                      # 排除 node_modules、xcode/build 等
  src/
    App.vue                       # 根空间组件
    scenes/
      index.vue                   # 默认窗口场景（包含 <spatial> 块示例）
    components/                   # 可复用空间组件目录
    composables/                  # Vue composables 目录
  xcode/
    Sources/
      App.swift                   # visionOS 应用入口（@main）
      VueSpatialRuntime/          # JS-Swift 桥接运行时框架
        Runtime.swift             # JS 引擎初始化与 Vue 运行时
  public/                         # 静态资源（3D 模型、纹理等）
```

### 5.2 关键文件内容说明

**alchemy.toml** 是项目的唯一真相来源。`package.json` 仅用于与现有 npm 工具链兼容（如 IDE 的包管理 UI）。`Package.swift` 是从 `alchemy.toml` 的 `[spatial-dependencies]` 和 `[spatial]` 段自动生成的产物，不应手动编辑。

**约定**：当开发者修改 `alchemy.toml` 中的 `[spatial-dependencies]` 后，运行 `alchemy install` 或 `alchemy spatial sync` 会自动重新生成 `Package.swift`。

---

## 6. 注册表支持

### 6.1 空间包元数据扩展

扩展 `alchemy_registry` 以支持空间包特有的元数据字段：

```rust
// crates/alchemy_registry/src/metadata.rs -- 扩展

/// 空间包额外元数据（包发布到 Alchemy 注册表时声明）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SpatialMetadata {
    /// 支持的平台列表
    #[serde(default)]
    pub platforms: Vec<String>,   // ["visionOS 2.0+", "iOS 18.0+"]

    /// 提供的场景类型
    #[serde(default)]
    pub scene_types: Vec<String>, // ["window", "immersive", "volume"]

    /// 是否包含 RealityKit 实体/组件
    #[serde(default)]
    pub has_reality_content: bool,

    /// 是否包含自定义 Swift 代码
    #[serde(default)]
    pub has_swift_code: bool,

    /// 需要的 Swift 依赖（桥接需求）
    #[serde(default)]
    pub required_swift_packages: Vec<String>,

    /// 需要的最低 Swift 工具链版本
    #[serde(default)]
    pub swift_tools_version: Option<String>,
}
```

### 6.2 注册表查询扩展

```rust
// 扩展 RegistryClient 以支持空间包搜索

impl RegistryClient {
    /// 搜索具有空间能力的包
    pub async fn search_spatial_packages(
        &self,
        query: &str,
        platform_filter: Option<&str>,
    ) -> AlchemyResult<Vec<SpatialPackageSearchResult>> {
        // 查询 Alchemy 注册表的空间包索引
        // GET /api/v1/spatial/search?q={query}&platform={platform}
        todo!()
    }
}

pub struct SpatialPackageSearchResult {
    pub name: String,
    pub version: String,
    pub description: String,
    pub spatial_metadata: SpatialMetadata,
}
```

### 6.3 CLI 搜索命令

```bash
# 搜索空间 Vue 组件包
$ alchemy search --spatial "3d-button"
Found 3 spatial packages:

  taraxacum-ui-3d    1.2.0    3D UI components for spatial Vue
    Platforms: visionOS 2.0+
    Scene types: window, volume

  spatial-controls   0.8.0    Gesture-based controls for visionOS
    Platforms: visionOS 2.0+, iOS 18.0+
    Scene types: window, immersive

  reality-widgets    0.3.0    RealityKit widget components
    Platforms: visionOS 2.0+
    Scene types: volume
    Requires Swift: RealityKit
```

---

## 7. RayRepo 集成

### 7.1 Monorepo 中的空间依赖协调

当项目使用 RayRepo 管理 monorepo 时，Alchemy 需要与 RayRepo 协调 Swift 依赖：

```
my-monorepo/                       # RayRepo workspace root
  rayrepo.yaml                     # RayRepo workspace 配置
  Package.swift                    # 统一的 Swift 包清单（Alchemy 生成）
  packages/
    shared-components/             # 共享 Vue 空间组件库
      alchemy.toml
      src/
    app-main/                      # 主应用
      alchemy.toml
      src/
    app-settings/                  # 设置应用
      alchemy.toml
      src/
```

### 7.2 Workspace 级 Package.swift 生成

```rust
// crates/alchemy_spatial/src/workspace.rs（未来模块）

/// 当在 RayRepo monorepo 中运行时:
/// 1. 收集所有子包的 spatial-dependencies
/// 2. 去重并合并版本约束
/// 3. 在 workspace 根生成统一的 Package.swift
/// 4. 每个子包作为一个 Swift target
pub fn generate_workspace_package_swift(
    workspace_root: &Path,
    packages: &[WorkspacePackage],
) -> Result<String, SpatialError> {
    // 合并所有子包的 spatial-dependencies
    let mut all_swift_deps: BTreeMap<String, SpatialDepSpec> = BTreeMap::new();

    for pkg in packages {
        for (name, spec) in &pkg.manifest.spatial_dependencies {
            if let Some(existing) = all_swift_deps.get(name) {
                // 版本冲突检测
                if !specs_compatible(existing, spec) {
                    return Err(SpatialError::VersionConflict(format!(
                        "Workspace 中 {} 的 Swift 依赖 {} 版本冲突: {} vs {}",
                        pkg.name, name,
                        format_spec(existing), format_spec(spec)
                    )));
                }
            }
            all_swift_deps.insert(name.clone(), spec.clone());
        }
    }

    // 生成统一 Package.swift
    // ...
    todo!()
}
```

### 7.3 协调流程

```
alchemy install (在 monorepo 根运行)
  |
  +-> 检测 rayrepo.yaml -> 进入 workspace 模式
  |
  +-> 遍历所有 packages/*/alchemy.toml
  |     |
  |     +-> 收集 JS dependencies        -> 统一 JS 解析
  |     +-> 收集 spatial-dependencies    -> 统一 Swift 解析
  |
  +-> 生成 workspace 根 Package.swift
  |
  +-> JS: node_modules 链接（per-package）
  +-> Swift: 统一 swift package resolve
  |
  +-> 写入 alchemy.lock（包含所有 JS + Swift 锁定版本）
```

---

## 8. 开发阶段

### 阶段 1: alchemy.toml spatial-dependencies 段解析 (1 周)

**目标**: 支持读取和验证 `alchemy.toml` 中的 `[spatial-dependencies]` 段。

**具体任务**:
- [ ] 在 `alchemy_core` 中新增 `spatial_manifest.rs`，实现 `AlchemyManifest` 及相关结构体
- [ ] 在 workspace 根 `Cargo.toml` 中添加 `toml = "0.8"` 依赖
- [ ] 修改 `alchemy_cli/commands/install.rs`，检测 `alchemy.toml` 存在时优先使用
- [ ] 修改 `alchemy_cli/commands/add.rs`，支持 `--spatial` 标志
- [ ] 编写单元测试：TOML 解析、各种 `SpatialDepSpec` 变体的反序列化
- [ ] 编写集成测试：从示例 `alchemy.toml` 文件读取并验证结构

**验收标准**: `alchemy install` 能正确读取 `alchemy.toml`，解析 JS 依赖段（`[dependencies]`），跳过但不报错于 `[spatial-dependencies]` 段。

### 阶段 2: alchemy_spatial crate -- Swift 包解析 (2 周)

**目标**: 实现 Swift 依赖解析的核心逻辑。

**具体任务**:
- [ ] 创建 `crates/alchemy_spatial/` 目录结构和 `Cargo.toml`
- [ ] 在 workspace 根 `Cargo.toml` 的 `members` 中注册新 crate
- [ ] 实现 `swift_resolver.rs`：
  - Git 仓库缓存管理（clone/fetch 到 `~/.alchemy-store/swift-repos/`）
  - 版本标签解析（列出 git tag，匹配 `from` 约束）
  - 系统框架识别（RealityKit, SwiftUI 等无需 clone）
  - 传递依赖递归解析
  - 版本冲突检测
- [ ] 实现 `platform.rs`：`SpatialPlatform` 解析和 SDK 可用性检测
- [ ] 编写单元测试：版本解析、平台解析、冲突检测

**验收标准**: `SwiftResolver` 能解析包含 Git 依赖和系统框架的 `spatial-dependencies`，生成 `SwiftResolutionResult`。

### 阶段 3: Package.swift 生成 (1 周)

**目标**: 从 `alchemy.toml` 自动生成合法的 `Package.swift` 文件。

**具体任务**:
- [ ] 实现 `swift_manifest.rs` 中的 `generate_package_swift()` 函数
- [ ] 实现 `parse_package_swift()`（基于 `swift package dump-package`）
- [ ] 添加 `alchemy spatial sync` 命令
- [ ] 确保生成的 `Package.swift` 能通过 `swift package resolve` 验证
- [ ] 处理边界情况：空依赖、只有系统框架、混合来源

**验收标准**: 从示例 `alchemy.toml` 生成的 `Package.swift` 可以被 `swift package resolve` 成功解析，且 `swift build` 不会因 manifest 错误而失败。

### 阶段 4: `alchemy spatial init` 脚手架 (1 周)

**目标**: 一键创建完整的空间 Vue 项目结构。

**具体任务**:
- [ ] 实现 `scaffold.rs` 中的全部文件生成
- [ ] 实现 `xcode.rs` 中的 App.swift 和 VueSpatialRuntime 模板生成
- [ ] 在 `alchemy_cli` 中注册 `Spatial` 子命令和 `SpatialCommands::Init`
- [ ] 交互式初始化（可选）：项目名称、目标平台、场景类型选择
- [ ] 确保生成的项目可以直接运行 `alchemy install` + `alchemy spatial build`

**验收标准**: `alchemy spatial init my-app && cd my-app && alchemy install` 成功完成，不产生错误。

### 阶段 5: `alchemy spatial build` + `run` 命令 (2 周)

**目标**: 实现空间项目的完整构建和运行流程。

**具体任务**:
- [ ] 实现 `build.rs` 中的 `SpatialBuilder`
- [ ] Vite 构建集成（调用 npx vite build）
- [ ] Package.swift 自动同步
- [ ] Swift 编译集成（调用 swift build / xcodebuild）
- [ ] JS bundle 嵌入 .app 资源目录
- [ ] 模拟器启动（`xcrun simctl` 或 `xcodebuild run`）
- [ ] `--watch` 模式：文件变更检测 + 增量重编译
- [ ] 实现 `alchemy spatial doctor`：工具链检查命令
- [ ] 实现 `alchemy spatial package`：生成 .xcarchive / .ipa

**验收标准**: 在安装了 Xcode 16+ 和 visionOS SDK 的 macOS 环境上，`alchemy spatial run` 能编译项目并在 visionOS Simulator 中启动应用。

### 阶段 6: 注册表空间元数据 (1 周)

**目标**: 扩展 Alchemy 注册表以支持空间包的发布和发现。

**具体任务**:
- [ ] 在 `alchemy_registry/src/metadata.rs` 中添加 `SpatialMetadata` 结构
- [ ] 扩展包发布 API，接受空间元数据
- [ ] 实现空间包搜索/过滤
- [ ] 在 `alchemy search` 中添加 `--spatial` 过滤器
- [ ] 在 `alchemy info` 中显示空间元数据

**验收标准**: 能发布一个带有空间元数据的包，并通过 `alchemy search --spatial` 找到它。

### 阶段 7: RayRepo 集成 (1 周)

**目标**: 在 monorepo 环境下协调多包的 Swift 依赖。

**具体任务**:
- [ ] 实现 workspace 级 Swift 依赖合并
- [ ] 实现 workspace 根 `Package.swift` 生成
- [ ] 依赖冲突检测和报告
- [ ] 与 RayRepo 的 workspace 协议对接
- [ ] 集成测试：多包 monorepo 场景

**验收标准**: 在包含 3 个空间子包的 RayRepo monorepo 中，`alchemy install` 能正确合并 Swift 依赖并生成统一的 `Package.swift`。

---

## 附录 A: 现有代码的必要修改清单

以下是对现有 crate 需要进行的最小修改，以支持空间计算功能：

| 文件 | 修改内容 |
|------|---------|
| `Cargo.toml` (workspace 根) | `members` 添加 `"crates/alchemy_spatial"` |
| `crates/alchemy_core/Cargo.toml` | 添加 `toml = "0.8"` 依赖 |
| `crates/alchemy_core/src/lib.rs` | 添加 `pub mod spatial_manifest;` |
| `crates/alchemy_core/src/error.rs` | 添加 `SpatialError` 变体和 `TomlError` |
| `crates/alchemy_core/src/lockfile.rs` | 添加 `spatial_packages` 字段到 `Lockfile` 结构体 |
| `crates/alchemy_cli/Cargo.toml` | 添加 `alchemy_spatial` 依赖 |
| `crates/alchemy_cli/src/main.rs` | 添加 `Spatial` 子命令和 `SpatialCommands` 枚举 |
| `crates/alchemy_cli/src/commands/mod.rs` | 添加 `pub mod spatial;` |
| `crates/alchemy_cli/src/commands/install.rs` | 检测 `alchemy.toml` 并分派双解析流程 |
| `crates/alchemy_cli/src/commands/add.rs` | 添加 `--spatial` 标志处理 |
| `crates/alchemy_registry/src/metadata.rs` | 添加 `SpatialMetadata` 结构体 |

## 附录 B: 错误类型扩展

```rust
// 在 crates/alchemy_core/src/error.rs 中扩展

#[derive(Error, Debug)]
pub enum AlchemyError {
    // ...现有变体保持不变...

    #[error("TOML 解析错误: {0}")]
    TomlParse(String),

    #[error("空间依赖错误: {0}")]
    SpatialDependency(String),

    #[error("Swift 工具链未找到: {0}")]
    SwiftToolchainMissing(String),

    #[error("平台不支持: {0}")]
    UnsupportedPlatform(String),

    #[error("Package.swift 生成失败: {0}")]
    PackageSwiftGeneration(String),
}
```

## 附录 C: 配置扩展

```rust
// 在 crates/alchemy_core/src/config.rs 的 AlchemyConfig 中扩展

pub struct AlchemyConfig {
    // ...现有字段保持不变...

    /// Swift 仓库缓存目录（默认 ~/.alchemy-store/swift-repos/）
    pub swift_cache_dir: PathBuf,
    /// 是否在 install 时自动同步 Package.swift
    pub auto_sync_package_swift: bool,
    /// 空间构建的默认目标平台
    pub spatial_default_destination: String,
}
```
