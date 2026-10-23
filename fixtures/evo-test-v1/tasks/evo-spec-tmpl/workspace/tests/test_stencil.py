"""Visible examples for SPEC.md. Run from the workspace root: python3 -m unittest discover -s tests

These cover only part of the specification; every rule of SPEC.md is checked by further tests."""

import unittest

from stencil import TemplateError, render


class OutputTest(unittest.TestCase):
    def test_text_and_output_tags(self):
        self.assertEqual(render("Hello, {= name =}!", {"name": "Ada"}), "Hello, Ada!")
        self.assertEqual(render("no tags at all", {}), "no tags at all")
        self.assertEqual(render("{=name=}{= name =}", {"name": "x"}), "xx")

    def test_paths(self):
        ctx = {"order": {"id": 17, "lines": [{"sku": "A-1"}, {"sku": "B-2"}]}}
        self.assertEqual(render("#{= order.id =}: {= order.lines.1.sku =}", ctx), "#17: B-2")

    def test_formatting(self):
        self.assertEqual(render("{= n =}|{= tags =}", {"n": -3, "tags": ["red", "blue"]}), "-3|red, blue")

    def test_escaping(self):
        self.assertEqual(render("<p>{= s =}</p>", {"s": "Tom & <Jerry>"}), "<p>Tom &amp; &lt;Jerry&gt;</p>")

    def test_comment(self):
        self.assertEqual(render("a{# note: {= skipped =} #}b", {}), "ab")


class ExpressionTest(unittest.TestCase):
    def test_default_operator(self):
        self.assertEqual(render('{= title ?? "Untitled" =}', {}), "Untitled")
        self.assertEqual(render('{= title ?? "Untitled" =}', {"title": "Notes"}), "Notes")

    def test_filters_chain(self):
        self.assertEqual(render("{= name | trim | upper =}", {"name": "  ada lovelace "}), "ADA LOVELACE")
        self.assertEqual(render("{= word | lower | length =}", {"word": "HELLO"}), "5")

    def test_filter_arguments(self):
        self.assertEqual(render('[{= code | pad 6 "0" =}]', {"code": 42}), "[000042]")
        self.assertEqual(render('{= parts | join "/" =}', {"parts": ["usr", "local", "bin"]}), "usr/local/bin")

    def test_group(self):
        self.assertEqual(render("{= n | group =}", {"n": 9876543}), "9,876,543")

    def test_slice(self):
        self.assertEqual(render("{= word | slice 0 3 =}", {"word": "template"}), "tem")

    def test_membership(self):
        tpl = '{: if "admin" in roles :}yes{: else :}no{: end :}'
        self.assertEqual(render(tpl, {"roles": ["user", "admin"]}), "yes")
        self.assertEqual(render(tpl, {"roles": ["user"]}), "no")


class BlockTest(unittest.TestCase):
    def test_if_elif_else(self):
        tpl = "{: if score > 89 :}A{: elif score > 79 :}B{: else :}C{: end :}"
        self.assertEqual(render(tpl, {"score": 95}), "A")
        self.assertEqual(render(tpl, {"score": 80}), "B")
        self.assertEqual(render(tpl, {"score": 12}), "C")

    def test_for_list_with_loop_variables(self):
        tpl = "{: for p in people :}{= #index =}/{= #count =} {= p.name =};{: end :}"
        ctx = {"people": [{"name": "Ann"}, {"name": "Bob"}]}
        self.assertEqual(render(tpl, ctx), "1/2 Ann;2/2 Bob;")

    def test_standalone_lines(self):
        tpl = "Items:\n{: for i in items :}\n- {= i =}\n{: end :}\nDone."
        self.assertEqual(render(tpl, {"items": ["pen", "ink"]}), "Items:\n- pen\n- ink\nDone.")

    def test_macro(self):
        tpl = '{: macro greet who :}Hello, {= who =}! {: end :}{: call greet "Ann" :}{: call greet name :}'
        self.assertEqual(render(tpl, {"name": "Bob"}), "Hello, Ann! Hello, Bob! ")

    def test_set(self):
        self.assertEqual(render('{: set greeting = "hi" :}{= greeting | upper =}!', {}), "HI!")


class ErrorTest(unittest.TestCase):
    def assertError(self, tpl, ctx, text):
        with self.assertRaises(TemplateError) as cm:
            render(tpl, ctx)
        self.assertEqual(str(cm.exception), text)

    def test_undefined_variable(self):
        self.assertError("Dear {= customer.name =},", {"customer": {}}, "1:6: undefined variable 'customer.name'")

    def test_unknown_filter(self):
        self.assertError("x\n  {= v | capitalize =}", {"v": "a"}, "2:3: unknown filter 'capitalize'")

    def test_stray_end(self):
        self.assertError("ok{: end :}", {}, "1:3: unexpected 'end'")


if __name__ == "__main__":
    unittest.main()
