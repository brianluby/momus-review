import hashlib
import hmac

def signature(payload, secret):
    return hmac.new(secret, payload, hashlib.sha256).hexdigest()
