# Catalog pagination
The stable page shape is {items, nextCursor}. Existing collector callers continue until nextCursor is null. The new hasMore field is additive.
