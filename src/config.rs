use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub pubsub: PubSubConfig,
    #[serde(default)]
    pub redis: RedisConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_instance_id")]
    pub instance_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PubSubConfig {
    pub project_id: String,
    pub subscription_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RedisConfig {
    /// Redis URL, e.g., "redis://localhost:6379" or "redis://:password@host:6379"
    /// 如果为空，则禁用消息重放功能
    #[serde(default)]
    pub url: Option<String>,
}

fn default_port() -> u16 {
    8080
}

fn default_instance_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl AppConfig {
    pub fn load() -> anyhow::Result<Self> {
        let config_path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.yaml".to_string());

        let mut config = if Path::new(&config_path).exists() {
            let content = std::fs::read_to_string(&config_path)?;
            serde_yaml::from_str(&content)?
        } else {
            Self {
                server: ServerConfig {
                    port: 8080,
                    instance_id: default_instance_id(),
                },
                pubsub: PubSubConfig {
                    project_id: String::new(),
                    subscription_id: String::new(),
                },
                redis: RedisConfig::default(),
            }
        };

        // 环境变量覆盖配置文件（优先级更高）
        if let Ok(port) = std::env::var("PORT") {
            if let Ok(p) = port.parse() {
                config.server.port = p;
            }
        }
        if let Ok(project) = std::env::var("GCP_PROJECT") {
            config.pubsub.project_id = project;
        }
        if let Ok(sub) = std::env::var("PUBSUB_SUBSCRIPTION") {
            config.pubsub.subscription_id = sub;
        }
        if let Ok(url) = std::env::var("REDIS_URL") {
            config.redis.url = Some(url);
        }

        // 验证必要配置
        if config.pubsub.project_id.is_empty() {
            anyhow::bail!("GCP_PROJECT environment variable is required");
        }
        if config.pubsub.subscription_id.is_empty() {
            anyhow::bail!("PUBSUB_SUBSCRIPTION environment variable is required");
        }

        Ok(config)
    }

    pub fn test_config() -> Self {
        Self {
            server: ServerConfig {
                port: std::env::var("PORT")
                    .ok()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(8080),
                instance_id: format!("test-{}", &default_instance_id()[..8]),
            },
            pubsub: PubSubConfig {
                project_id: "test-project".to_string(),
                subscription_id: "test-subscription".to_string(),
            },
            redis: RedisConfig {
                url: std::env::var("REDIS_URL").ok(),
            },
        }
    }
}
