def timeout_seconds(config):
    if "timeout_ms" in config:
        return config["timeout_ms"] / 1000
    return config.get("timeout", 10)
