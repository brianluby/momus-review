exports.updateTitle = (rows, tenant, id, title) => {
  const row = rows.find(row => row.id === id);
  if (!row) return false;
  row.title = title;
  return true;
};
