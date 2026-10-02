import hashlib
import hmac

def signature(payload, secret):
    return hmac.new(secret, payload, hashlib.sha256).hexdigest()

def valid_webhook(payload, supplied, secret):
    expected = signature(payload, secret)
    return hmac.compare_digest(expected, supplied)
