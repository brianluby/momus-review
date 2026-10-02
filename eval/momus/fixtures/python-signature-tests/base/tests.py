from webhook import signature
assert len(signature(b"event", b"secret")) == 64
