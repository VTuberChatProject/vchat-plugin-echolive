//! Echo-Live 官方配置历史的版本边界与路径选择，保持资源声明版本。
use serde_json::{Value, json};
use std::path::Path;

pub(crate) const MAX_DATA_VERSION: u64 = 17;

/// 版本定义按发布时的文件名优先探测，其余名称兼容原样搬迁的资源目录。
pub(crate) fn definition_path(root: &Path, version: u64) -> Option<&'static str> {
    let mut files = [
        "res/script/config_define.js",
        "res/script/config-define.js",
        "res/data/config_define.js",
    ];
    files.swap(
        0,
        if version >= 7 {
            2
        } else if version >= 3 {
            1
        } else {
            0
        },
    );
    files.into_iter().find(|file| root.join(file).is_file())
}

/// v1 尚未提供定义文件；元数据取自官方 1.2.7 原配置，保留上游字段拼写。
pub(crate) fn v1_definitions() -> Value {
    serde_json::from_str::<Value>(include_str!("v1_fields.json"))
        .expect("内置 v1 配置元数据")
        ["fields"]
        .clone()
}

/// 1.6.2～1.6.4 的声明仍为 11，实际定义版本为 12；覆盖层只使用有效字段。
pub(crate) fn effective_version(config: u64, schema: Option<u64>) -> u64 {
    if config == 11 && schema == Some(12) {
        12
    } else {
        config
    }
}

/// 依据 created/deleted 选择发布时可用字段；负值为官方预留占位项。
pub(crate) fn active(definition: &Value, version: u64) -> bool {
    definition["created"]
        .as_i64()
        .is_none_or(|created| created >= 0 && created as u64 <= version)
        && definition["deleted"]
            .as_u64()
            .is_none_or(|deleted| version < deleted)
}

/// 旧版本引用 from 原路径；重分组及无障碍更名分别从 v4、v10 起生效。
pub(crate) fn field_path(definition: &Value, version: u64) -> Option<&str> {
    let name = definition["name"].as_str()?;
    let Some(from) = definition["from"].as_str() else {
        return Some(name);
    };
    let changed = if name.starts_with("accessibility.") {
        10
    } else {
        4
    };
    if version < changed {
        Some(from)
    } else {
        Some(name)
    }
}

/// 描述编辑能力与播放能力，让历史版本保持自身的数据契约。
pub(crate) fn compatibility(version: Option<u64>, effective: Option<u64>) -> String {
    match version {
        None => "config.js 缺少有效 data_version；配置以只读方式保留".into(),
        Some(v) if v == 0 || v > MAX_DATA_VERSION => format!("data_version {v} 超出已适配范围 1～17；配置以只读方式保留"),
        Some(11) if effective == Some(12) => "兼容 1.6.2～1.6.4：声明版本 11，字段定义版本 12；保持声明版本。历史版本的播放回执能力按页面能力检测".into(),
        Some(v) if v < MAX_DATA_VERSION => format!("已适配 data_version {v}，保持原版本；历史版本的播放回执能力按页面能力检测"),
        _ => "已适配 data_version 17".into(),
    }
}

/// 无定义的配置仍可作为资源接入，并在设置页显示只读原因。
pub(crate) fn read_only_schema(version: &Value) -> Value {
    json!({
        "version": 1, "data_version": version, "schema_version": null,
        "effective_version": version, "read_only": true,
        "compatibility": compatibility(version.as_u64(), version.as_u64()), "fields": [],
    })
}
