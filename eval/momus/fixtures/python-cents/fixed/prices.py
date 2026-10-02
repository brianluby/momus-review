from decimal import Decimal

def parse_cents(value):
    return int(Decimal(value) * 100)
