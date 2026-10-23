"""Registry: a name -> target table. A target is a callable or a "package.module:attribute" string, imported
the first time the name is used (so a plugin costs nothing until it is needed)."""
import importlib

from ..errors import RegistryError


def resolve(target):
    """The object a target names."""
    if callable(target):
        return target
    if not isinstance(target, str) or ":" not in target:
        raise RegistryError("bad plugin target %r (want module:attribute)" % (target,))
    module_name, _, attr = target.partition(":")
    try:
        obj = importlib.import_module(module_name)
        for part in attr.split("."):
            obj = getattr(obj, part)
    except (ImportError, AttributeError) as e:
        raise RegistryError("cannot load %s: %s" % (target, e))
    if not callable(obj):
        raise RegistryError("%s is not callable" % target)
    return obj


class Registry:
    def __init__(self, kind):
        self.kind = kind
        self._targets = {}
        self._loaded = {}

    def register(self, name, target, replace=False) -> None:
        if name in self._targets and not replace:
            raise RegistryError("%s %r is already registered" % (self.kind, name))
        self._targets[name] = target
        self._loaded.pop(name, None)

    def names(self) -> list:
        return sorted(self._targets)

    def __contains__(self, name):
        return name in self._targets

    def target(self, name):
        return self._targets.get(name)

    def get(self, name):
        if name not in self._targets:
            raise RegistryError("unknown %s: %s" % (self.kind, name))
        if name not in self._loaded:
            self._loaded[name] = resolve(self._targets[name])
        return self._loaded[name]
