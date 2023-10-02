# Python 3.12 Sub-interpreters

These are some safe-ish Rust bindings to the new CFFI API for creating and managing sub-interpreters
within Python.


### Prerequisites 
You need at least Python 3.12+ (duh)

To build and mess with this project you need to have Rust installed which you can get at:

https://www.rust-lang.org/tools/install

### Installation
Once this is installed you can run:

```shell
pipenv install
pipenv shell
maturin develop
```

Which will build and compile the library which can then be imported as `subinterpreters`.

### Usage

It's a pretty simple API:

```py
from subinterpreters import create_interpreter, SubInterpreter


new: SubInterpreter = create_interpreter(
    # Some optional config options to mess with:
    # allow_exec=False,
    # allow_fork=False,
    # allow_threads=True,
    # allow_daemon_threads=False,
)

new.run_code(
    """
    import random
    print(random.randint(1, 10))
    """
)

# You can pass globals and locals
new.run_code(
    """
    import random
    print(random.randint(1, 10))
    """,
    globals=globals(),
    locals=locals(),
)
```

### Notes

Sometimes, if an error occurs, the process will exit with a status code which largely 
means "Some memory fucked up" without it being an actual segfault. I don't know why, as far as
I can tell this is correct. But I do know there is still the limitation that sub-interpreters aren't
cleaned up quite correctly by Python if they're still running when the main interpreter shuts down.
