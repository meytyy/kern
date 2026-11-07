using System;
using System.Collections.Generic;

namespace Kern.Demo;

/// <summary>Greets people.</summary>
[Serializable]
public record Person(string Name, int Age);

public static class Program
{
    // Entry point.
    public static async Task<int> Main(string[] args)
    {
        var people = new List<Person> { new("Ada", 36) };
        foreach (var p in people)
        {
            Console.WriteLine($"Hello {p.Name}, you are {p.Age}!");
        }
        await Task.Delay(10);
        return Helper(args.Length) ? 0 : 1;
    }

    [Obsolete("use Helper2")]
    private static bool Helper(int count) => count > 0 && true;
}
