from webhook import signature, valid_webhook
payload, secret = b"event", b"secret"
signed = signature(payload, secret)
assert valid_webhook(payload, signed, secret)
assert not valid_webhook(b"tampered", signed, secret)
assert not valid_webhook(payload, "0" * 64, secret)
