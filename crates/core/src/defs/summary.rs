// i[impl plan.dry-run]
//
// `ResourceSummary` is a serialisable, comparable view of a `Resource` used by
// the dry-run RPC to compute field-level diffs between the current and
// proposed AppDefs. The summary captures the operator-meaningful surface of
// each resource type — image, scale, mounts, env, etc. — without exposing
// internal Rhai bookkeeping (FnPtrs, closures, weak refs).
//
// Top-level field names of these structs are the field names that appear in
// the dry-run `fields[]` output. Keep the field set stable; renaming changes
// the operator-facing API.

use std::collections::BTreeMap;

use serde::Serialize;

use super::{
    container::{ContainerDef, VolumeMount},
    deployment::Deployment,
    enums::{OnExit, OnTerminate, OnUpdate},
    ingress::{HttpTermination, Ingress, RedirectDef},
    job::Job,
    pod::{HttpBinding, PodDef, TcpUdpBinding},
    resource::Resource,
    service::{HttpService, Service},
    volume::{ExternalVolume, Volume},
};

#[derive(Serialize, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResourceSummary {
    Service(ServiceSummary),
    HttpService(HttpServiceSummary),
    Ingress(IngressSummary),
    Deployment(DeploymentSummary),
    Job(JobSummary),
    Volume(VolumeSummary),
    ExternalVolume(ExternalVolumeSummary),
}

#[derive(Serialize, Debug, PartialEq)]
pub struct ServiceSummary {
    pub http: bool,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct HttpServiceSummary {
    pub service: String,
    pub port: u16,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct IngressSummary {
    pub service: String,
    pub hostname: String,
    pub port: u16,
    pub tls: bool,
    pub dtls: bool,
    pub http_terminate: Option<&'static str>,
    pub redirect: Option<RedirectSummary>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct RedirectSummary {
    pub port: u16,
    pub code: u16,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct DeploymentSummary {
    pub container: ContainerSummary,
    pub pod: PodSummary,
    pub scale: ScaleSummary,
    pub on_update: &'static str,
    pub on_terminate: &'static str,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct JobSummary {
    pub container: ContainerSummary,
    pub pod: PodSummary,
    pub deadline: Option<u64>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct ContainerSummary {
    pub image: Option<String>,
    pub command: Option<Vec<String>>,
    pub args: Option<Vec<String>>,
    pub env: BTreeMap<String, String>,
    pub volume_mounts: BTreeMap<String, VolumeMountSummary>,
    pub on_exit: &'static str,
    pub memory: Option<String>,
    pub cpus: Option<f64>,
    pub extra_caps: Vec<String>,
    pub writable_rootfs: bool,
    pub pids_limit: Option<u32>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VolumeMountSummary {
    Volume { name: Option<String> },
    ExternalVolume { name: String },
}

#[derive(Serialize, Debug, PartialEq)]
pub struct PodSummary {
    /// Service-port mounts, encoded as `service-name:port` for stable diffing.
    pub service_mounts: Vec<String>,
    /// HTTP bindings, encoded as `pod-port -> service-name route`.
    pub http_bindings: Vec<String>,
    /// TCP bindings, encoded as `pod-port -> service-name:port`.
    pub tcp_bindings: Vec<String>,
    /// UDP bindings, encoded as `pod-port -> service-name:port`.
    pub udp_bindings: Vec<String>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct ScaleSummary {
    pub low: u16,
    pub high: u16,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct VolumeSummary {
    pub readonly: bool,
    pub tmpfs: bool,
    pub writes: BTreeMap<String, String>,
    pub exported: bool,
    pub export_description: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct ExternalVolumeSummary {}

// ---------------------------------------------------------------------------
// Resource → Summary
// ---------------------------------------------------------------------------

impl Resource {
    pub fn summary(&self) -> ResourceSummary {
        match self {
            Self::Service(s) => ResourceSummary::Service(s.summary()),
            Self::HttpService(h) => ResourceSummary::HttpService(h.summary()),
            Self::Ingress(i) => ResourceSummary::Ingress(i.summary()),
            Self::Deployment(d) => ResourceSummary::Deployment(d.summary()),
            Self::Job(j) => ResourceSummary::Job(j.summary()),
            Self::Volume(v) => ResourceSummary::Volume(v.summary()),
            Self::ExternalVolume(v) => ResourceSummary::ExternalVolume(v.summary()),
        }
    }
}

impl Service {
    pub fn summary(&self) -> ServiceSummary {
        let def = self.def.lock();
        ServiceSummary {
            http: def.http.is_some(),
        }
    }
}

impl HttpService {
    pub fn summary(&self) -> HttpServiceSummary {
        HttpServiceSummary {
            service: self.service.name.as_str().to_owned(),
            port: self.port.get(),
        }
    }
}

impl Ingress {
    pub fn summary(&self) -> IngressSummary {
        let def = self.def.lock();
        IngressSummary {
            service: self.service.name.as_str().to_owned(),
            hostname: def.hostname.clone(),
            port: def.port.get(),
            tls: def.tls,
            dtls: def.dtls,
            http_terminate: def.http_terminate.as_ref().map(|t| match t {
                HttpTermination::Http1 => "http1",
                HttpTermination::Http2 => "http2",
            }),
            redirect: def.redirect.as_ref().map(RedirectDef::summary),
        }
    }
}

impl RedirectDef {
    pub fn summary(&self) -> RedirectSummary {
        RedirectSummary {
            port: self.port.get(),
            code: self.code,
        }
    }
}

impl Deployment {
    pub fn summary(&self) -> DeploymentSummary {
        let def = self.def.lock();
        let pod = def.pod.lock();
        let container = pod.container.lock();
        DeploymentSummary {
            container: container.summary(),
            pod: pod.summary(),
            scale: ScaleSummary {
                low: def.scale.start,
                high: def.scale.end,
            },
            on_update: match def.on_update {
                OnUpdate::Rolling => "rolling",
                OnUpdate::Replace => "replace",
            },
            on_terminate: match def.on_terminate {
                OnTerminate::Recreate => "recreate",
            },
        }
    }
}

impl Job {
    pub fn summary(&self) -> JobSummary {
        let def = self.def.lock();
        let pod = def.pod.lock();
        let container = pod.container.lock();
        JobSummary {
            container: container.summary(),
            pod: pod.summary(),
            deadline: def.deadline,
        }
    }
}

impl PodDef {
    pub fn summary(&self) -> PodSummary {
        let mut service_mounts: Vec<String> = self
            .service_mounts
            .iter()
            .map(|sp| format!("{}:{}", sp.service.name.as_str(), sp.port.get()))
            .collect();
        service_mounts.sort();

        let mut http_bindings: Vec<String> = self
            .http_bindings
            .iter()
            .map(HttpBinding::summary)
            .collect();
        http_bindings.sort();

        let mut tcp_bindings: Vec<String> = self
            .tcp_bindings
            .iter()
            .map(TcpUdpBinding::summary)
            .collect();
        tcp_bindings.sort();

        let mut udp_bindings: Vec<String> = self
            .udp_bindings
            .iter()
            .map(TcpUdpBinding::summary)
            .collect();
        udp_bindings.sort();

        PodSummary {
            service_mounts,
            http_bindings,
            tcp_bindings,
            udp_bindings,
        }
    }
}

impl HttpBinding {
    pub fn summary(&self) -> String {
        format!(
            "{} -> {} {}",
            self.pod_port.get(),
            self.route.http.service.name.as_str(),
            self.route.prefix,
        )
    }
}

impl TcpUdpBinding {
    pub fn summary(&self) -> String {
        format!(
            "{} -> {}:{}",
            self.pod_port.get(),
            self.service_port.service.name.as_str(),
            self.service_port.port.get(),
        )
    }
}

impl ContainerDef {
    pub fn summary(&self) -> ContainerSummary {
        let env: BTreeMap<String, String> = self
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let volume_mounts: BTreeMap<String, VolumeMountSummary> = self
            .volume_mounts
            .iter()
            .map(|(path, mount)| (path.to_string_lossy().into_owned(), mount.summary()))
            .collect();
        ContainerSummary {
            image: self.image.clone(),
            command: self.command.clone(),
            args: self.args.clone(),
            env,
            volume_mounts,
            on_exit: match self.on_exit {
                None => "default",
                Some(OnExit::Restart) => "restart",
                Some(OnExit::Terminate) => "terminate",
                Some(OnExit::RestartOnFailure) => "restart_on_failure",
            },
            memory: self.memory.clone(),
            cpus: self.cpus,
            extra_caps: self.extra_caps.clone(),
            writable_rootfs: self.writable_rootfs,
            pids_limit: self.pids_limit,
        }
    }
}

impl VolumeMount {
    pub fn summary(&self) -> VolumeMountSummary {
        match self {
            Self::Volume(v) => VolumeMountSummary::Volume {
                name: v.name.as_ref().map(|n| n.as_str().to_owned()),
            },
            Self::ExternalVolume(v) => VolumeMountSummary::ExternalVolume {
                name: v.name.as_str().to_owned(),
            },
        }
    }
}

impl Volume {
    pub fn summary(&self) -> VolumeSummary {
        let def = self.def.lock();
        let writes: BTreeMap<String, String> = def
            .writes
            .iter()
            .map(|(path, content)| (path.clone(), content.clone()))
            .collect();
        VolumeSummary {
            readonly: def.read_only,
            tmpfs: def.tmpfs,
            writes,
            exported: def.exported.is_some(),
            export_description: def
                .exported
                .as_ref()
                .and_then(|opts| opts.description.clone()),
        }
    }
}

impl ExternalVolume {
    pub fn summary(&self) -> ExternalVolumeSummary {
        ExternalVolumeSummary {}
    }
}

// ---------------------------------------------------------------------------
// Diff helper
// ---------------------------------------------------------------------------

/// Returns the names of top-level fields that differ between two
/// `ResourceSummary` values. Both summaries must be of the same variant; the
/// `kind` discriminator is ignored.
pub fn diff_fields(current: &ResourceSummary, proposed: &ResourceSummary) -> Vec<String> {
    let cur = serde_json::to_value(current).unwrap_or(serde_json::Value::Null);
    let prop = serde_json::to_value(proposed).unwrap_or(serde_json::Value::Null);
    let (Some(cur), Some(prop)) = (cur.as_object(), prop.as_object()) else {
        return Vec::new();
    };
    let mut keys: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for k in cur.keys().chain(prop.keys()) {
        if k != "kind" {
            keys.insert(k.as_str());
        }
    }
    keys.into_iter()
        .filter(|k| cur.get(*k) != prop.get(*k))
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests;
