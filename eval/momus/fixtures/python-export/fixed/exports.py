def resolve_export(root, name):
    root = root.resolve()
    target = (root / name).resolve()
    target.relative_to(root)
    return target
