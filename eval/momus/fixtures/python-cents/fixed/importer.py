from prices import parse_cents

def import_price(row):
    return {"cents": parse_cents(row["price"])}
