// Test-only helpers for the libvmx reference build.
//
// * vmxref_set_avx2 lets the conformance tests force the 128-bit code path on
//   x86_64 (the AVX2 path is chosen at runtime).
// * vmxref_set_threads / vmxref_destroy replace VMX_SetThreads / VMX_Destroy.
//   Upstream ThreadTask::Destroy can lose its wake-up (running=false is set
//   and notified while the worker is between its `while (running)` check and
//   `cv.wait`), which hangs thread.join() forever. We keep notifying from a
//   helper thread until the worker has exited.
#include "vmxcodec.h"
#include "thread_tasks.h"
#include <atomic>
#include <chrono>
#include <thread>

extern "C" void vmxref_set_avx2(VMX_INSTANCE* instance, int enabled)
{
	if (instance) instance->avx2 = enabled;
}

static void safe_destroy_tasks(ThreadTasks* tasks)
{
	if (!tasks) return;
	for (int i = 0; i < tasks->numThreads; i++)
	{
		ThreadTask* task = tasks->tasks[i];
		{
			std::lock_guard<std::mutex> lock(task->mtx);
			task->running = false;
		}
		std::atomic<bool> joined{ false };
		std::thread kicker([&] {
			while (!joined.load())
			{
				task->cv.notify_all();
				std::this_thread::sleep_for(std::chrono::milliseconds(1));
			}
		});
		task->thread.join();
		joined = true;
		kicker.join();
		delete task;
	}
	delete[] tasks->tasks;
	delete tasks;
}

extern "C" void vmxref_set_threads(VMX_INSTANCE* instance, int n)
{
	if (!instance || n <= 0) return;
	safe_destroy_tasks(instance->Tasks);
	instance->Threads = n;
	instance->Tasks = CreateTasks(n);
}

extern "C" void vmxref_destroy(VMX_INSTANCE* instance)
{
	if (!instance) return;
	safe_destroy_tasks(instance->Tasks);
	instance->Tasks = NULL;
	VMX_Destroy(instance);
}
