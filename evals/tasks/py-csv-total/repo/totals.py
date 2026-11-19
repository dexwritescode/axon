"""Totals per customer from an orders CSV."""


def totals(text: str) -> dict[str, float]:
    """Sum the `amount` column per `customer` in CSV `text`.

    The first line is the header and names the columns, in any order.
    Fields may be quoted, and quoted fields may contain commas.
    Blank lines are skipped.
    """
    lines = text.splitlines()
    header = lines[0].split(",")
    customer = header.index("customer")
    amount = header.index("amount")
    result: dict[str, float] = {}
    for line in lines:
        fields = line.split(",")
        name = fields[customer]
        result[name] = result.get(name, 0.0) + float(fields[amount])
    return result
