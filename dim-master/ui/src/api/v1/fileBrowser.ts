import v1 from "./";

export const fileBrowser = v1.injectEndpoints({
  endpoints: (build) => ({
    getDirectories: build.query<string[], string>({
      query: (path) => path ? `filebrowser/${path}` : `filebrowser`,
    }),
  }),
});

export const { useGetDirectoriesQuery } = fileBrowser;

export default fileBrowser;
