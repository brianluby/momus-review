from exports import resolve_export

def download_path(root, query):
    return resolve_export(root, query["name"])
