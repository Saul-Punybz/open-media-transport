// Test harness around upstream libomtnet, used to observe its real behaviour.
//
//   send NAME SECONDS          announce NAME and send 640x360 UYVY video (30 fps),
//                              stereo audio with a silent right channel, per-frame
//                              metadata every 30th frame, and sender info
//   recv ADDRESS SECONDS [compressed]
//                              connect to "MACHINE (Name)" or omt://host:port, set
//                              tally to program and quality to High, print every frame
//   list SECONDS               print what discovery finds
//
// One line per event on stdout, so runs can be diffed.

using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using libomtnet;

static class Program
{
    static int Main(string[] args)
    {
        if (args.Length < 2) return Usage();
        OMTLogging.SetCallback(line => Console.Error.WriteLine("log: " + line.TrimEnd()));
        switch (args[0])
        {
            case "send" when args.Length >= 3: return Send(args[1], int.Parse(args[2]));
            case "recv" when args.Length >= 3: return Recv(args[1], int.Parse(args[2]), args.Length > 3 && args[3] == "compressed");
            case "list": return List(int.Parse(args[1]));
            default: return Usage();
        }
    }

    static int Usage()
    {
        Console.Error.WriteLine("usage: send NAME SECONDS | recv ADDRESS SECONDS [compressed] | list SECONDS");
        return 2;
    }

    static int Send(string name, int seconds)
    {
        const int w = 640, h = 360, fps = 30, rate = 48000, channels = 2;
        int samples = rate / fps;
        using var send = new OMTSend(name, OMTQuality.Default);
        send.SetSenderInformation(new OMTSenderInfo("omt-harness", "open-media-transport", "0.1"));
        send.AddConnectionMetadata("<HarnessHello Value=\"1\" />");
        Console.WriteLine($"send address={send.Address} url={send.URL} port={send.Port}");

        IntPtr video = Marshal.AllocHGlobal(w * 2 * h);
        IntPtr audio = Marshal.AllocHGlobal(samples * channels * 4);
        byte[] meta = Encoding.UTF8.GetBytes("<HarnessFrame N=\"0\" />\0");
        IntPtr metaPtr = Marshal.AllocHGlobal(256);
        try
        {
            var sw = Stopwatch.StartNew();
            var lastTally = new OMTTally();
            for (int n = 0; sw.Elapsed.TotalSeconds < seconds; n++)
            {
                FillUyvy(video, w, h, n);
                var vf = new OMTMediaFrame
                {
                    Type = OMTFrameType.Video, Timestamp = -1, Codec = (int)OMTCodec.UYVY,
                    Width = w, Height = h, Stride = w * 2, FrameRateN = fps, FrameRateD = 1,
                    AspectRatio = 16f / 9f, ColorSpace = OMTColorSpace.BT709,
                    Data = video, DataLength = w * 2 * h,
                };
                if (n % 30 == 0)
                {
                    meta = Encoding.UTF8.GetBytes($"<HarnessFrame N=\"{n}\" />\0");
                    Marshal.Copy(meta, 0, metaPtr, meta.Length);
                    vf.FrameMetadata = metaPtr;
                    vf.FrameMetadataLength = meta.Length;
                }
                send.Send(vf);

                FillAudio(audio, samples, n);
                var af = new OMTMediaFrame
                {
                    Type = OMTFrameType.Audio, Timestamp = -1, Codec = (int)OMTCodec.FPA1,
                    SampleRate = rate, Channels = channels, SamplesPerChannel = samples,
                    Data = audio, DataLength = samples * channels * 4,
                };
                send.Send(af);

                // GetTally(0, ...) always returns false (OMTSendReceiveBase.cs:60-75); compare values.
                var tally = new OMTTally();
                send.GetTally(0, ref tally);
                if (tally.Preview != lastTally.Preview || tally.Program != lastTally.Program)
                {
                    Console.WriteLine($"send tally preview={tally.Preview} program={tally.Program}");
                    lastTally = tally;
                }
            }
            Console.WriteLine($"send done connections={send.Connections}");
        }
        finally
        {
            Marshal.FreeHGlobal(video);
            Marshal.FreeHGlobal(audio);
            Marshal.FreeHGlobal(metaPtr);
        }
        return 0;
    }

    static int Recv(string address, int seconds, bool compressedOnly)
    {
        var flags = compressedOnly ? OMTReceiveFlags.CompressedOnly : OMTReceiveFlags.None;
        using var recv = new OMTReceive(address, OMTFrameType.Video | OMTFrameType.Audio | OMTFrameType.Metadata,
            OMTPreferredVideoFormat.UYVY, flags);
        recv.SetTally(new OMTTally(0, 1));
        recv.SetSuggestedQuality(OMTQuality.High);
        var sw = Stopwatch.StartNew();
        int video = 0, audio = 0;
        while (sw.Elapsed.TotalSeconds < seconds)
        {
            // A fresh struct each time: libomtnet leaves FrameMetadata untouched on
            // frames without metadata (OMTReceive.cs:1059-1072).
            var frame = new OMTMediaFrame();
            if (!recv.Receive(OMTFrameType.Video | OMTFrameType.Audio | OMTFrameType.Metadata, 200, ref frame)) continue;
            string fm = frame.FrameMetadataLength > 0 ? Printable(frame.FrameMetadata, frame.FrameMetadataLength) : "";
            switch (frame.Type)
            {
                case OMTFrameType.Video:
                    if (video++ % 30 == 0 || fm != "")
                        Console.WriteLine($"recv video ts={frame.Timestamp} {frame.Width}x{frame.Height} codec={FourCC(frame.Codec)} flags={(int)frame.Flags} cs={(int)frame.ColorSpace} rate={frame.FrameRateN}/{frame.FrameRateD} compressed={frame.CompressedLength} meta=\"{fm}\"");
                    break;
                case OMTFrameType.Audio:
                    if (audio++ % 30 == 0)
                        Console.WriteLine($"recv audio ts={frame.Timestamp} rate={frame.SampleRate} ch={frame.Channels} spc={frame.SamplesPerChannel} len={frame.DataLength}");
                    break;
                case OMTFrameType.Metadata:
                    Console.WriteLine($"recv metadata ts={frame.Timestamp} len={frame.DataLength} xml=\"{Printable(frame.Data, frame.DataLength)}\"");
                    break;
            }
        }
        var info = recv.GetSenderInformation();
        Console.WriteLine($"recv done video={video} audio={audio} info={(info == null ? "none" : info.ProductName + "/" + info.Manufacturer + "/" + info.Version)}");
        return 0;
    }

    static int List(int seconds)
    {
        var d = OMTDiscovery.GetInstance();
        Thread.Sleep(seconds * 1000);
        foreach (var a in d.GetAddresses()) Console.WriteLine($"list \"{a}\"");
        return 0;
    }

    static void FillUyvy(IntPtr dst, int w, int h, int n)
    {
        var row = new byte[w * 2];
        for (int y = 0; y < h; y++)
        {
            for (int x = 0; x < w; x += 2)
            {
                int i = x * 2;
                row[i] = 128; row[i + 1] = (byte)((x + n) & 255);
                row[i + 2] = 128; row[i + 3] = (byte)((y + n) & 255);
            }
            Marshal.Copy(row, 0, dst + y * w * 2, row.Length);
        }
    }

    static void FillAudio(IntPtr dst, int samples, int n)
    {
        var buf = new float[samples * 2];
        for (int i = 0; i < samples; i++) buf[i] = (float)Math.Sin(2 * Math.PI * 1000 * (n * samples + i) / 48000.0) * 0.25f;
        Marshal.Copy(buf, 0, dst, buf.Length); // channel 1 (right) left as zeros
    }

    static string FourCC(int c) => Encoding.ASCII.GetString(BitConverter.GetBytes(c));

    static string Printable(IntPtr p, int len)
    {
        var b = new byte[len];
        Marshal.Copy(p, b, 0, len);
        var sb = new StringBuilder();
        foreach (byte x in b) sb.Append(x >= 32 && x < 127 && x != '"' ? ((char)x).ToString() : $"\\x{x:X2}");
        return sb.ToString();
    }
}
