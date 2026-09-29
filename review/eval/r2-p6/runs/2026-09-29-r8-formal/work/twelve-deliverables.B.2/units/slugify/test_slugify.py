from slugify import Impl
I = Impl()
def test_slug():
    assert I.slugify("Hello, World!") == "hello-world"
    assert I.slugify("  --A@@B-- ") == "a-b"
    assert I.slugify("***") == "untitled"
