"""Exporters turn a list of items into the text of an external file. They are looked up by name through the
registry: csv, tsv (the picking list), json, ledger. An exporter writes the items it is given, in that order;
selecting and sorting is the caller's job (exporters.common.select)."""
