"""Stores: where an Inventory lives between commands.

    CsvStore   a directory holding items.csv and movements.csv (the default)
    JsonStore  a single inventory.json-style file (any path ending in .json)

storage.factory.open_store(path) picks one by the path's shape.
"""
