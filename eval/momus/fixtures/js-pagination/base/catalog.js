exports.page = (rows, cursor) => ({items: rows.slice(cursor, cursor + 2), nextCursor: cursor + 2 < rows.length ? cursor + 2 : null});
