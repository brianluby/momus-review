# Signed webhook
Signatures are HMAC-SHA256 hexadecimal strings. valid_webhook must accept the signed payload and reject a modified payload or incorrect signature. New verifier tests must cover rejection as well as acceptance.
