"""Importers turn the text of an external file into an ImportResult (items with their line numbers, and row
errors). They are looked up by name through the registry: csv, json, jsonl, ledger.

An importer never touches a store: services.merge applies a result to an inventory.
"""
