// Test harness around upstream libomtnet, used to observe its real behaviour.
//
//   send NAME SECONDS [uyvy|uyva|p216|pa16] [T=ADDRESS ...]
//                              announce NAME and send 640x360 video (30 fps; UYVY by
//                              default, or UYVA/PA16 with alpha, or 10-bit P216),
//                              stereo audio with a silent right channel, per-frame
//                              metadata every 30th frame, and sender info; at T
//                              seconds call SetRedirect(ADDRESS) (empty clears it)
//   recv ADDRESS SECONDS [compressed|preview|-] [FORMAT]
//                              connect to "MACHINE (Name)" or omt://host:port, set
//                              tally to program and quality to High, print every frame
//                              and every change of RedirectAddress; FORMAT is an
//                              OMTPreferredVideoFormat name (default UYVY)
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
            case "send" when args.Length >= 3:
                return Send(args[1], int.Parse(args[2]), Array.Find(args[3..], a => !a.Contains('=')) ?? "uyvy",
                    Array.FindAll(args[3..], a => a.Contains('=')));
            case "recv" when args.Length >= 3: return Recv(args[1], int.Parse(args[2]), args.Length > 3 ? args[3] : "",
                args.Length > 4 ? Enum.Parse<OMTPreferredVideoFormat>(args[4], true) : OMTPreferredVideoFormat.UYVY);
            case "list": return List(int.Parse(args[1]));
            default: return Usage();
        }
    }

    static int Usage()
    {
        Console.Error.WriteLine("usage: send NAME SECONDS [uyvy|uyva|p216|pa16] [T=ADDRESS ...] | recv ADDRESS SECONDS [compressed|preview|-] [FORMAT] | list SECONDS");
        return 2;
    }

    static int Send(string name, int seconds, string format, string[] schedule)
    {
        var redirects = new System.Collections.Generic.List<(double, string)>();
        foreach (var e in schedule)
        {
            int eq = e.IndexOf('=');
            redirects.Add((double.Parse(e[..eq], System.Globalization.CultureInfo.InvariantCulture), e[(eq + 1)..]));
        }
        redirects.Sort((a, b) => a.Item1.CompareTo(b.Item1));
        int nextRedirect = 0;
        const int w = 640, h = 360, fps = 30, rate = 48000, channels = 2;
        // Bytes of the whole picture and the OMT codec for each source format.
        var (codec, length, flags) = format switch
        {
            "uyva" => (OMTCodec.UYVA, w * 2 * h + w * h, OMTVideoFlags.Alpha),
            "p216" => (OMTCodec.P216, w * 2 * h * 2, OMTVideoFlags.None),
            "pa16" => (OMTCodec.PA16, w * 2 * h * 3, OMTVideoFlags.Alpha),
            _ => (OMTCodec.UYVY, w * 2 * h, OMTVideoFlags.None),
        };
        int samples = rate / fps;
        using var send = new OMTSend(name, OMTQuality.Default);
        send.SetSenderInformation(new OMTSenderInfo("omt-harness", "open-media-transport", "0.1"));
        send.AddConnectionMetadata("<HarnessHello Value=\"1\" />");
        Console.WriteLine($"send address={send.Address} url={send.URL} port={send.Port}");

        IntPtr video = Marshal.AllocHGlobal(length);
        IntPtr audio = Marshal.AllocHGlobal(samples * channels * 4);
        byte[] meta = Encoding.UTF8.GetBytes("<HarnessFrame N=\"0\" />\0");
        IntPtr metaPtr = Marshal.AllocHGlobal(256);
        try
        {
            var sw = Stopwatch.StartNew();
            var lastTally = new OMTTally();
            for (int n = 0; sw.Elapsed.TotalSeconds < seconds; n++)
            {
                while (nextRedirect < redirects.Count && sw.Elapsed.TotalSeconds >= redirects[nextRedirect].Item1)
                {
                    string to = redirects[nextRedirect++].Item2;
                    send.SetRedirect(to == "" ? null : to);
                    Console.WriteLine($"send redirect t={sw.Elapsed.TotalSeconds:F1} address=\"{to}\"");
                }
                if (codec == OMTCodec.P216 || codec == OMTCodec.PA16) Fill16(video, w, h, n, codec == OMTCodec.PA16);
                else FillUyvy(video, w, h, n, codec == OMTCodec.UYVA);
                var vf = new OMTMediaFrame
                {
                    Type = OMTFrameType.Video, Timestamp = -1, Codec = (int)codec,
                    Width = w, Height = h, Stride = w * 2, FrameRateN = fps, FrameRateD = 1,
                    AspectRatio = 16f / 9f, ColorSpace = OMTColorSpace.BT709, Flags = flags,
                    Data = video, DataLength = length,
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

    static int Recv(string address, int seconds, string mode, OMTPreferredVideoFormat format)
    {
        var flags = mode == "compressed" ? OMTReceiveFlags.CompressedOnly
            : mode == "preview" ? OMTReceiveFlags.Preview : OMTReceiveFlags.None;
        using var recv = new OMTReceive(address, OMTFrameType.Video | OMTFrameType.Audio | OMTFrameType.Metadata,
            format, flags);
        recv.SetTally(new OMTTally(0, 1));
        recv.SetSuggestedQuality(OMTQuality.High);
        var sw = Stopwatch.StartNew();
        int video = 0, audio = 0;
        string redirect = null;
        while (sw.Elapsed.TotalSeconds < seconds)
        {
            if (recv.RedirectAddress != redirect)
            {
                redirect = recv.RedirectAddress;
                Console.WriteLine($"recv redirect t={sw.Elapsed.TotalSeconds:F1} address=\"{redirect}\"");
            }
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
                    if (fm != "" && frame.DataLength > 0)
                        Console.WriteLine($"pixels {fm} fnv1a64={Fnv1a64(frame.Data, frame.DataLength):x16} stride={frame.Stride}");
                    break;
                case OMTFrameType.Audio:
                    if (audio++ % 30 == 0)
                        Console.WriteLine($"recv audio ts={frame.Timestamp} rate={frame.SampleRate} ch={frame.Channels} spc={frame.SamplesPerChannel} len={frame.DataLength} fnv1a64={Fnv1a64(frame.Data, frame.DataLength):x16}");
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

    static void FillUyvy(IntPtr dst, int w, int h, int n, bool alpha)
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
        if (!alpha) return;
        // UYVA: an 8-bit alpha plane after the picture.
        var a = new byte[w];
        for (int y = 0; y < h; y++)
        {
            for (int x = 0; x < w; x++) a[x] = (byte)((x + 2 * y + n) & 255);
            Marshal.Copy(a, 0, dst + w * 2 * h + y * w, w);
        }
    }

    // P216 (PA16 with alpha): 16-bit samples, 10 significant bits in the high bits.
    // Luma and chroma ramps with steps finer than 8 bits can hold.
    static void Fill16(IntPtr dst, int w, int h, int n, bool alpha)
    {
        var row = new short[w];
        int plane = w * 2 * h;
        for (int y = 0; y < h; y++)
        {
            for (int x = 0; x < w; x++) row[x] = (short)((64 + (x + 3 * y + 5 * n) % 877) << 6);
            Marshal.Copy(row, 0, dst + y * w * 2, w);
            for (int x = 0; x < w; x += 2)
            {
                row[x] = (short)((64 + (2 * x + n) % 897) << 6);
                row[x + 1] = (short)((64 + (2 * y + n) % 897) << 6);
            }
            Marshal.Copy(row, 0, dst + plane + y * w * 2, w);
            if (!alpha) continue;
            for (int x = 0; x < w; x++) row[x] = (short)(((3 * x + y + n) % 1024) << 6);
            Marshal.Copy(row, 0, dst + 2 * plane + y * w * 2, w);
        }
    }

    static void FillAudio(IntPtr dst, int samples, int n)
    {
        var buf = new float[samples * 2];
        for (int i = 0; i < samples; i++) buf[i] = (float)Math.Sin(2 * Math.PI * 1000 * (n * samples + i) / 48000.0) * 0.25f;
        Marshal.Copy(buf, 0, dst, buf.Length); // channel 1 (right) left as zeros
    }

    // FNV-1a 64 of the decoded pixels, to compare with other receivers.
    static ulong Fnv1a64(IntPtr p, int len)
    {
        var b = new byte[len];
        Marshal.Copy(p, b, 0, len);
        ulong h = 0xcbf29ce484222325;
        foreach (byte x in b) { h ^= x; h *= 0x100000001b3; }
        return h;
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
