pub struct RuntimeConfig {
    pub host: String,
    pub port: u16,
}

impl RuntimeConfig {
    pub fn from_env() -> Self {
        let host = std::env::var("DLOAD_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
        let port = match std::env::var("DLOAD_PORT") {
            Ok(v) => v.parse::<u16>().unwrap_or_else(|_| {
                eprintln!("DLOAD_PORT '{}' is not a valid port number; falling back to 8080", v);
                8080
            }),
            Err(_) => 8080,
        };
        Self { host, port }
    }
}
