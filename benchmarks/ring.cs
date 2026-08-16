// 100,000 tasks chained by channels, passing a counter through all of them.
using System;
using System.Threading.Channels;
using System.Threading.Tasks;

class Ring {
    static async Task Hop(ChannelReader<int> inp, ChannelWriter<int> outp) =>
        await outp.WriteAsync(await inp.ReadAsync() + 1);

    static async Task Main() {
        var first = Channel.CreateUnbounded<int>();
        var inp = first;
        for (int i = 0; i < 100000; i++) {
            var outp = Channel.CreateUnbounded<int>();
            _ = Hop(inp.Reader, outp.Writer);
            inp = outp;
        }
        _ = first.Writer.WriteAsync(0).AsTask();
        Console.WriteLine(await inp.Reader.ReadAsync());
    }
}
