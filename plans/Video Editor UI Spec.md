* The goal is to create a UI for the use of Claude along with the video editor console application (cutline) to prepare videos.
* The UI should allow for integration with Claude Code (installed on the local machine).
* Claude Code is required for this version to work.
* It should work with Claude Code to take user input in markdown form as a spec for the video to be created.
* Markdown Editor code can be reused from "'/Users/danieljebaraj/Documents/git/gitea/Just Notes'".
    * Support images.
    * Support mermaid diagrams.
    * Anything that will help provide clear context for the video to be created.
    * We want to make the system as expressive as possible.
* It should also be able to enhance the markdown with feedback from the user using Claude code as well as in the editor. 
    * It should be possible to specify the model to be used for help with editing the spec. Sonnet, Fable etc and version or latest.
    * It should be possible to gather additional information from the user with queries from Claude Code.
* If video and other files are to be provided for use with the edit there should be an option to add these. These are retained where they are and used by reference. They are not copied over.
* Once there is a complete spec, the use should be able to submit the job to Claude Code for completion.
* Pre-submission extensive validation should occur so that any failure possibility or quality issues are caught before an expensive video production run.
* It should be possible to specify a different version of Claude Model for the actual video production.
* Integration with elevenlabs should be provided for voice over and background music (API).
    * https://elevenlabs.io/
* Any supporting media files are referred and a project file can be saved and re-opened for easy work, cloning etc.
    * The video spec is stored on disk as simple markdown embedded in the project file.
* If the project file is moved, missing media items should be easy to update to a new location from the UI itself. For example when moved to a new machine a file may not be in the same relative or absolute location. It should be possible to easily correct this by selecting the required file from the UI itself.
* It should be possible to specify the intent of the video.
    * Marketing etc.
* The system should also allow for common brand assets such as logos, theme colors etc to be saved as a preset within the application itself. 
    * Multiple presets should be supported.
    * It should be easy to switch between presets.
* The UI should allow user friendly options for the video output. The user should be able to easily specify what they want for the run. Multiple config settings should also be allowed to switch between different runs.
    * YouTube
    * LinkedIn
    * Shorts on YouTube
    * Apple App Store etc
    * All other common social media channels
* It should be possible to invoke the application to run a config and produce output on the command line. Such use should provide proper updates on progress, allow cancellation etc.
* Each run should produce detailed logs that can then be analyzed to make fixes as need to the Video Editor code. These logs should be easy to access and download.
* Output folders should be easy to specify and update.
* UI should be professional looking suitable for business use.
* Current target is Mac only but write in such a way that it is possible to port to Windows with https://github.com/microsoft/windows-rs later.