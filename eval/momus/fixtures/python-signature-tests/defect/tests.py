from webhook import signature, valid_webhook
payload, secret = b"event", b"secret"
signed = signature(payload, secret)
assert valid_webhook(payload, signed, secret)
