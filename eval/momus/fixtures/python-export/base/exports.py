def resolve_export(root, name):
    if "/" in name:
        raise ValueError("flat export names only")
    return root / name
