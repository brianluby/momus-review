def timeout_seconds(config):
    return config.get("timeout_ms", 10000) / 1000
