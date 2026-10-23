"""Stencil: a small template language. SPEC.md (next to this package) is the contract."""

from .engine import TemplateError, render

__all__ = ["render", "TemplateError"]
