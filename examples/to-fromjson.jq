[range(.) | tojson] | join(",") | "[" + . + "]" | fromjson
