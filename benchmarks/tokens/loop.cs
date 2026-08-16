long[] v = new long[1024];
for (int i = 0; i < 1024; i++) v[i] = i;
long acc = 0;
for (long i = 0; i < 100_000_000; i++) acc += v[i & 1023];
System.Console.WriteLine(acc);
