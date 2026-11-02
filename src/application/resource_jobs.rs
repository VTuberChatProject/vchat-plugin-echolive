//! ABI 外运行的资源准备任务；进度与日志通过独立同步状态交回宿主。
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

static NEXT_JOB: AtomicU64 = AtomicU64::new(1);

struct Job {
    id: String,
    state: Arc<Mutex<Value>>,
    cancelled: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

#[derive(Default)]
pub(crate) struct Jobs {
    job: Option<Job>,
    logs: Arc<Mutex<VecDeque<String>>>,
}

impl Jobs {
    /// 线程只捕获拥有所有权的数据，完全独立于插件 Context 和 HostApi。
    pub(crate) fn start(
        &mut self,
        assets: PathBuf,
        action: String,
        path: Option<PathBuf>,
    ) -> Result<Value, String> {
        if self
            .job
            .as_ref()
            .is_some_and(|job| job.state.lock().unwrap_or_else(|e| e.into_inner())["busy"] == true)
        {
            return Err("已有 Echo-Live 资源准备任务，请等待完成或取消".into());
        }
        if let Some(worker) = self.job.as_mut().and_then(|job| job.worker.take()) {
            let _ = worker.join();
        }
        if !matches!(action.as_str(), "folder" | "import" | "download") {
            return Err(format!("未知资源获取操作：{action}"));
        }
        if action != "download" && path.is_none() {
            return Err("资源准备操作缺少路径".into());
        }
        let id = format!(
            "resource-{}-{}",
            std::process::id(),
            NEXT_JOB.fetch_add(1, Ordering::Relaxed)
        );
        let initial = json!({"version": 1, "id": id, "stage": "queued", "bytes": 0,
            "total": null, "busy": true, "resource": null});
        let state = Arc::new(Mutex::new(initial.clone()));
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_state = state.clone();
        let worker_cancelled = cancelled.clone();
        let logs = self.logs.clone();
        log(
            &logs,
            format!(
                "资源任务 {id} 开始：action={action}, path={}",
                path.as_ref()
                    .map_or_else(String::new, |path| path.display().to_string())
            ),
        );
        let job_id = id.clone();
        let worker = std::thread::Builder::new()
            .name("echolive-resources".into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::resources::prepare(
                        &assets,
                        &action,
                        path.as_deref(),
                        &worker_cancelled,
                        |stage, bytes, total| {
                            let mut state = worker_state.lock().unwrap_or_else(|e| e.into_inner());
                            if state["stage"].as_str() != Some(stage) {
                                log(&logs, format!("资源任务 {job_id} 阶段：{stage}"));
                            }
                            state["stage"] = json!(stage);
                            state["bytes"] = json!(bytes);
                            state["total"] = json!(total);
                        },
                    )
                }))
                .unwrap_or_else(|_| Err("资源准备线程意外终止".into()));
                let mut state = worker_state.lock().unwrap_or_else(|e| e.into_inner());
                state["busy"] = json!(false);
                match outcome {
                    Ok(resource) if !worker_cancelled.load(Ordering::Acquire) => {
                        state["stage"] = json!("ready");
                        log(
                            &logs,
                            format!(
                                "资源任务 {job_id} 准备完成：path={}, data_version={}, schema_version={}, compatibility_status={}, sha256={}",
                                resource["path"].as_str().unwrap_or(""), resource["data_version"],
                                resource["schema_version"], resource["compatibility_status"], resource["sha256"]
                            ),
                        );
                        state["resource"] = resource;
                    }
                    outcome => {
                        let error = outcome.err().unwrap_or_else(|| "资源准备已取消".into());
                        state["failed_stage"] = state["stage"].clone();
                        state["stage"] = json!(if worker_cancelled.load(Ordering::Acquire) {
                            "cancelled"
                        } else {
                            "failed"
                        });
                        state["error"] = json!(error);
                        log(&logs, format!("资源任务 {job_id} 结束：{error}"));
                    }
                }
            })
            .map_err(|e| format!("启动资源任务线程: {e}"))?;
        self.job = Some(Job {
            id,
            state,
            cancelled,
            worker: Some(worker),
        });
        Ok(initial)
    }

    /// 快照查询保持 ABI 即时响应；任务编号隔离旧设置窗口的轮询。
    pub(crate) fn status(&self, id: &str) -> Result<Value, String> {
        let job = self
            .job
            .as_ref()
            .filter(|job| job.id == id)
            .ok_or("资源任务已失效，请重新获取状态")?;
        Ok(job.state.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    pub(crate) fn cancel(&self, id: Option<&str>) -> Result<Value, String> {
        let Some(job) = self.job.as_ref() else {
            return Ok(json!({"version": 1, "busy": false, "stage": "idle"}));
        };
        if id.is_some_and(|id| id != job.id) {
            return Err("资源任务编号已失效".into());
        }
        let mut state = job.state.lock().unwrap_or_else(|e| e.into_inner());
        if state["busy"] == true || state["stage"] == "ready" {
            job.cancelled.store(true, Ordering::Release);
            state["stage"] = json!(if state["busy"] == true {
                "cancelling"
            } else {
                "cancelled"
            });
            state["resource"] = Value::Null;
            log(&self.logs, format!("资源任务 {} 收到取消请求", job.id));
        }
        Ok(state.clone())
    }

    pub(crate) fn cancel_all(&self) {
        let _ = self.cancel(None);
    }

    /// 卸载动态库前等待线程退出，保证所有正在执行的代码仍位于已加载模块。
    pub(crate) fn shutdown(&mut self) {
        self.cancel_all();
        if let Some(worker) = self.job.as_mut().and_then(|job| job.worker.take()) {
            let _ = worker.join();
        }
    }

    pub(crate) fn drain_logs(&self) -> Vec<String> {
        self.logs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect()
    }
}

impl Drop for Jobs {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn log(logs: &Mutex<VecDeque<String>>, message: String) {
    let mut logs = logs.lock().unwrap_or_else(|e| e.into_inner());
    if logs.len() == 256 {
        logs.pop_front();
    }
    logs.push_back(message);
}
