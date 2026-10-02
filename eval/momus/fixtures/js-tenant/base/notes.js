exports.get = (rows, tenant, id) => rows.find(row => row.tenant === tenant && row.id === id);
