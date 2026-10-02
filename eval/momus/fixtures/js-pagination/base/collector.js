const catalog = require("./catalog");
exports.firstCursor = rows => catalog.page(rows, 0).nextCursor;
