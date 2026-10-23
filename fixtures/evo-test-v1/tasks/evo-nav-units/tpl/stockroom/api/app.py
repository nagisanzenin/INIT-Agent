"""App: loads the inventory, runs the matched handler, saves when the handler changed something, and maps
errors to statuses:

    HttpError          its own status
    NotFound           404   {"error": "unknown sku: X"}
    ValidationError    422   {"error": "invalid item", "problems": ["<subject> <field>: <message>", ...]}
#@A
    UnitError          422   {"error": "<message>"}
#@E
    DuplicateError     409
    StockError         409
    RegistryError      404
    other StockroomError, ValueError   400
"""
from ..config import load_config
from ..errors import DuplicateError, NotFound, RegistryError, StockError, StockroomError, ValidationError
#@A
from ..errors import UnitError
#@E
from . import router
from .http import HttpError, error_response


class Context:
    """What a handler gets besides the request: the loaded inventory, the config, and a dirty flag the
    handler sets when the inventory must be saved."""

    def __init__(self, inv, config, store=None):
        self.inv = inv
        self.config = config
        self.store = store
        self.dirty = False


class App:
    def __init__(self, store, config=None):
        self.store = store
        self.config = config or load_config()

    def handle(self, request):
        try:
            handler, params = router.match(request.method, request.path)
            ctx = Context(self.store.load(), self.config, self.store)
            response = handler(ctx, request, **params)
            if ctx.dirty:
                self.store.save(ctx.inv)
            return response
        except HttpError as e:
            return error_response(e.status, e.message)
        except NotFound as e:
            return error_response(404, str(e))
        except ValidationError as e:
            return error_response(422, "invalid item", problems=[str(p) for p in e.problems])
#@A
        except UnitError as e:
            return error_response(422, str(e))
#@E
        except (DuplicateError, StockError) as e:
            return error_response(409, str(e))
        except RegistryError as e:
            return error_response(404, str(e))
        except (StockroomError, ValueError) as e:
            return error_response(400, str(e))
