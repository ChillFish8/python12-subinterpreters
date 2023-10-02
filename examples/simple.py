from subinterpreters import create_interpreter, SubInterpreter


new: SubInterpreter = create_interpreter()

new.run_code(
    """
    import random
    print(random.randint(1, 10))
    """
)
